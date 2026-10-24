//! Servers sharing rooms, presence and invites through NATS.
//!
//! - Every server says it's alive twice a second. One that's been quiet for
//!   [`PEER_TIMEOUT`] counts as gone: its players drop out of rooms on other
//!   servers, and its rooms close for players elsewhere.
//! - A room runs on the server that made it. Players on other servers send
//!   their keys there and get its views back (see [`rooms`]).
//! - Private rooms' codes are kept in a NATS key-value bucket, so every
//!   server can find them.
//!
//! Messages to send go through one queue, so they leave in order.

mod rooms;
pub mod wire;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_nats::jetstream::{self, kv};
use async_nats::{Client, ConnectOptions, Subscriber};
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use self::rooms::{Export, Import};
use self::wire::{Alive, decode, encode};
use crate::config::ClusterConfig;
use crate::room::{LocalRooms, RoomRef, ServerId};

/// How often a server says it's alive.
pub const HEARTBEAT: Duration = Duration::from_millis(500);
/// How long a server may be quiet before it counts as gone.
pub const PEER_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for another server's answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for every server's answer, when asking them all.
const GATHER_WAIT: Duration = Duration::from_millis(500);
/// How often players and rooms whose server has gone are cleared away.
const SWEEP_EVERY: Duration = Duration::from_millis(250);
/// Messages waiting to be sent; more are dropped.
const OUTBOX: usize = 1 << 16;
/// Codes are written again this often, and the bucket forgets ones this
/// much older, so codes of a server that died don't pile up.
pub const CODES_REFRESH: Duration = Duration::from_secs(10 * 60);
const CODES_MAX_AGE: Duration = Duration::from_secs(30 * 60);

pub struct Cluster {
    pub server: ServerId,
    pub region: String,
    /// The start of every subject: `snaketui.<cluster name>`.
    prefix: String,
    client: Client,
    /// Private rooms' codes, each to a [`RoomRef`].
    pub(crate) codes: kv::Store,
    /// When each other server last said it was alive.
    peers: Mutex<HashMap<ServerId, Instant>>,
    outbox: mpsc::Sender<(String, Bytes)>,
    rooms: LocalRooms,
    /// Rooms here with players from other servers.
    exports: Mutex<HashMap<crate::room::RoomId, Export>>,
    /// Rooms elsewhere with players here.
    imports: Mutex<HashMap<RoomRef, Import>>,
}

impl Cluster {
    /// Connects to NATS as `server`, and starts answering other servers
    /// about the rooms in `rooms`.
    pub async fn connect(
        cfg: &ClusterConfig,
        server: ServerId,
        rooms: LocalRooms,
    ) -> Result<Arc<Self>> {
        let client = ConnectOptions::new()
            .name(format!("snaketui-server {server:016x}"))
            .request_timeout(Some(REQUEST_TIMEOUT))
            .connect(&cfg.nats)
            .await
            .context("connecting to NATS")?;
        let codes = jetstream::new(client.clone())
            .create_or_update_key_value(kv::Config {
                bucket: format!("snaketui_{}_codes", cfg.name),
                description: "Private rooms' codes, each to the server and room".into(),
                history: 1,
                max_age: CODES_MAX_AGE,
                storage: jetstream::stream::StorageType::File,
                ..Default::default()
            })
            .await
            .context("opening the room codes in NATS (is JetStream on?)")?;
        let (outbox, queued) = mpsc::channel(OUTBOX);
        let cluster = Arc::new(Self {
            server,
            region: cfg.region.clone(),
            prefix: format!("snaketui.{}", cfg.name),
            client,
            codes,
            peers: Mutex::default(),
            outbox,
            rooms,
            exports: Mutex::default(),
            imports: Mutex::default(),
        });
        tokio::spawn(Arc::clone(&cluster).send_queued(queued));
        cluster.listen_for_peers().await?;
        cluster.serve_rooms().await?;
        tokio::spawn(Arc::clone(&cluster).keep_alive());
        // Hear from the others once before saying which rooms are open.
        tokio::time::sleep(HEARTBEAT + HEARTBEAT / 2).await;
        info!(
            server = %format!("{server:016x}"),
            peers = cluster.peers().len(),
            "joined the cluster"
        );
        Ok(cluster)
    }

    pub fn subject(&self, tail: &str) -> String {
        format!("{}.{tail}", self.prefix)
    }

    /// Queues `msg` for `subject`. Messages leave in the order they're
    /// queued; if the queue is full, this one is dropped.
    pub fn send(&self, subject: String, msg: &impl Serialize) {
        if self.outbox.try_send((subject, encode(msg))).is_err() {
            debug!("the cluster's outbox is full; dropping a message");
        }
    }

    /// Asks whoever answers on `subject`; `None` if nobody does in time.
    pub async fn request<R: DeserializeOwned>(
        &self,
        subject: String,
        msg: &impl Serialize,
    ) -> Option<R> {
        match self.client.request(subject, encode(msg)).await {
            Ok(reply) => decode(&reply.payload),
            Err(e) => {
                debug!(error = %e, "no answer from the cluster");
                None
            }
        }
    }

    /// Asks every other server on `subject`, and collects the answers until
    /// all of them have answered or [`GATHER_WAIT`] has passed. This server
    /// should ignore its own question.
    pub async fn ask_all<R: DeserializeOwned>(
        &self,
        subject: String,
        msg: &impl Serialize,
    ) -> Vec<R> {
        let expected = self.peers().len();
        if expected == 0 {
            return Vec::new();
        }
        let inbox = self.inbox();
        let Ok(mut answers) = self.client.subscribe(inbox.clone()).await else {
            return Vec::new();
        };
        if let Err(e) = self
            .client
            .publish_with_reply(subject, inbox, encode(msg))
            .await
        {
            warn!(error = %e, "couldn't ask the other servers");
            return Vec::new();
        }
        let mut got = Vec::with_capacity(expected);
        let deadline = tokio::time::sleep(GATHER_WAIT);
        tokio::pin!(deadline);
        while got.len() < expected {
            tokio::select! {
                answer = answers.next() => match answer {
                    Some(answer) => got.extend(decode(&answer.payload)),
                    None => break,
                },
                _ = &mut deadline => break,
            }
        }
        got
    }

    pub async fn subscribe(&self, subject: String) -> Result<Subscriber> {
        Ok(self.client.subscribe(subject).await?)
    }

    /// A fresh subject for answers to come back on.
    pub fn inbox(&self) -> String {
        self.client.new_inbox()
    }

    /// Whether `server` has said it's alive lately. This server always is.
    pub fn alive(&self, server: ServerId) -> bool {
        server == self.server
            || self
                .peers
                .lock()
                .unwrap()
                .get(&server)
                .is_some_and(|seen| seen.elapsed() < PEER_TIMEOUT)
    }

    /// The other servers that are alive.
    pub fn peers(&self) -> HashSet<ServerId> {
        self.peers
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, seen)| seen.elapsed() < PEER_TIMEOUT)
            .map(|(server, _)| *server)
            .collect()
    }

    async fn send_queued(self: Arc<Self>, mut queued: mpsc::Receiver<(String, Bytes)>) {
        while let Some((subject, payload)) = queued.recv().await {
            if let Err(e) = self.client.publish(subject, payload).await {
                warn!(error = %e, "couldn't send to NATS");
            }
        }
    }

    async fn listen_for_peers(self: &Arc<Self>) -> Result<()> {
        let mut heard = self.subscribe(self.subject("alive")).await?;
        let me = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(msg) = heard.next().await {
                if let Some(Alive { server }) = decode(&msg.payload)
                    && server != me.server
                {
                    let new = me
                        .peers
                        .lock()
                        .unwrap()
                        .insert(server, Instant::now())
                        .is_none_or(|seen| seen.elapsed() >= PEER_TIMEOUT);
                    if new {
                        info!(server = %format!("{server:016x}"), "a server joined");
                    }
                }
            }
        });
        Ok(())
    }

    /// Says this server is alive, and clears away what servers that went
    /// quiet left behind.
    async fn keep_alive(self: Arc<Self>) {
        let alive = self.subject("alive");
        let mut beat = tokio::time::interval(HEARTBEAT);
        let mut sweep = tokio::time::interval(SWEEP_EVERY);
        loop {
            tokio::select! {
                _ = beat.tick() => self.send(alive.clone(), &Alive { server: self.server }),
                _ = sweep.tick() => {
                    let peers = self.peers();
                    let gone: Vec<ServerId> = {
                        let mut seen = self.peers.lock().unwrap();
                        let gone = seen.keys().filter(|s| !peers.contains(s)).copied().collect();
                        seen.retain(|s, _| peers.contains(s));
                        gone
                    };
                    for server in gone {
                        info!(server = %format!("{server:016x}"), "a server went quiet");
                    }
                    self.sweep_rooms(&peers);
                }
            }
        }
    }
}
