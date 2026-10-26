//! The test harness: a real server on a local port, played through russh's
//! SSH client, with what each player would see read back by a terminal
//! emulator.

// Each test file uses its own subset of these helpers.
#![allow(dead_code)]

pub mod accounts;
pub mod process;
pub mod rooms;

use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use russh::client::{self, KeyboardInteractiveAuthResponse, Msg};
use russh::keys::{Algorithm, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg};
use snaketui_server::config::ServerConfig;
use snaketui_server::input::STATUS_QUERY;
use snaketui_server::{serve, ssh};
use tokio::time::{Instant, timeout};

/// A server on `127.0.0.1:<free port>` with its own host key and database.
pub struct TestServer {
    pub addr: SocketAddr,
    /// Health and metrics, over HTTP.
    pub admin: SocketAddr,
    pub dir: PathBuf,
    pub stop: Option<tokio::sync::oneshot::Sender<()>>,
    pub thread: Option<std::thread::JoinHandle<()>>,
}

impl TestServer {
    pub fn start() -> Self {
        Self::with(|_| {})
    }

    pub fn with(tweak: impl FnOnce(&mut ServerConfig)) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "snaketui-e2e-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self::in_dir(dir, tweak)
    }

    pub fn in_dir(dir: PathBuf, tweak: impl FnOnce(&mut ServerConfig)) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let admin_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let admin = admin_listener.local_addr().unwrap();
        let mut cfg = ServerConfig {
            listen: addr,
            host_key: dir.join("host_key"),
            database: format!("sqlite://{}", dir.join("db.sqlite").display()),
            workers: 2,
            // Tests that stop servers mid-round would wait for it.
            drain_timeout_secs: 0,
            ..ServerConfig::default()
        };
        tweak(&mut cfg);
        let host_key = ssh::load_or_create_host_key(&cfg.host_key).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let until_stopped = async {
                let _ = stopped.await;
            };
            actix_rt::System::new()
                .block_on(serve(
                    cfg,
                    listener,
                    Some(admin_listener),
                    host_key,
                    until_stopped,
                ))
                .unwrap();
        });
        Self {
            addr,
            admin,
            dir,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    /// Stops the server and waits until it has finished.
    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }

    /// Stops the server, then serves again with the same host key and
    /// database. The old one gives up its directory, so dropping it removes
    /// nothing.
    pub fn restart(mut self) -> Self {
        self.stop();
        Self::in_dir(std::mem::take(&mut self.dir), |_| {})
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn new_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

pub struct TrustAnyHost;

impl client::Handler for TrustAnyHost {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

pub type Client = client::Handle<TrustAnyHost>;

#[derive(Clone, Copy)]
pub enum Login<'a> {
    Key(&'a PrivateKey),
    Guest,
    /// A guest who typed `ssh <name>@host`.
    GuestNamed(&'a str),
}

pub async fn connect(addr: SocketAddr) -> Client {
    client::connect(Arc::new(client::Config::default()), addr, TrustAnyHost)
        .await
        .unwrap()
}

pub async fn sign_in(client: &mut Client, login: Login<'_>) -> bool {
    match login {
        Login::Key(key) => {
            let key = PrivateKeyWithHashAlg::new(Arc::new(key.clone()), None);
            client
                .authenticate_publickey("alice", key)
                .await
                .unwrap()
                .success()
        }
        Login::Guest | Login::GuestNamed(_) => {
            let user = match login {
                Login::GuestNamed(name) => name,
                _ => "alice",
            };
            matches!(
                client
                    .authenticate_keyboard_interactive_start(user, None)
                    .await
                    .unwrap(),
                KeyboardInteractiveAuthResponse::Success
            )
        }
    }
}

/// One player's channel, and their screen as a terminal would show it.
pub struct Player {
    pub _client: Client,
    pub channel: Channel<Msg>,
    pub screen: vt100::Parser,
    pub exit_status: Option<u32>,
    pub closed: bool,
    pub received: usize,
}

impl Player {
    /// Signs in, asks for a `cols`×`rows` terminal and opens the game.
    pub async fn join(addr: SocketAddr, login: Login<'_>, cols: u32, rows: u32) -> Self {
        let mut client = connect(addr).await;
        assert!(sign_in(&mut client, login).await);
        Self::open(client, cols, rows).await
    }

    pub async fn open(client: Client, cols: u32, rows: u32) -> Self {
        let channel = client.channel_open_session().await.unwrap();
        channel
            .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .unwrap();
        channel.request_shell(true).await.unwrap();
        Self::on(client, channel, cols, rows)
    }

    pub fn on(client: Client, channel: Channel<Msg>, cols: u32, rows: u32) -> Self {
        let size = |n: u32| NonZeroU16::new(n as u16).unwrap();
        Self {
            _client: client,
            channel,
            screen: vt100::Parser::new(size(rows), size(cols), 0),
            exit_status: None,
            closed: false,
            received: 0,
        }
    }

    pub async fn send(&self, bytes: &[u8]) {
        self.channel.data(bytes).await.unwrap();
    }

    pub fn text(&self) -> String {
        self.screen.screen().contents()
    }

    /// Shows what the server sent, and answers its status queries the way
    /// a terminal does.
    async fn take(&mut self, data: &[u8]) {
        self.received += data.len();
        self.screen.process(data);
        let asked = data
            .windows(STATUS_QUERY.len())
            .filter(|w| *w == STATUS_QUERY)
            .count();
        for _ in 0..asked {
            let _ = self.channel.data(&b"\x1b[0n"[..]).await;
        }
    }

    /// Reads from the server until `done`, failing after `limit`.
    pub async fn wait_until(&mut self, what: &str, limit: Duration, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + limit;
        while !done(self) {
            assert!(
                !self.closed,
                "closed while waiting for {what}:\n{}",
                self.text()
            );
            let Ok(msg) = timeout(deadline - Instant::now(), self.channel.wait()).await else {
                panic!(
                    "timed out waiting for {what} after {} bytes; the screen shows:\n{}",
                    self.received,
                    self.text()
                );
            };
            match msg {
                Some(ChannelMsg::Data { data }) => self.take(&data).await,
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    self.exit_status = Some(exit_status)
                }
                Some(ChannelMsg::Close) | None => self.closed = true,
                Some(_) => {}
            }
        }
    }

    /// Like [`Self::wait_until`] for `text`, but says whether it showed up
    /// rather than failing.
    pub async fn saw(&mut self, text: &str, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while !self.text().contains(text) {
            if self.closed {
                return false;
            }
            let Ok(msg) = timeout(deadline - Instant::now(), self.channel.wait()).await else {
                return false;
            };
            match msg {
                Some(ChannelMsg::Data { data }) => self.take(&data).await,
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    self.exit_status = Some(exit_status)
                }
                Some(ChannelMsg::Close) | None => self.closed = true,
                Some(_) => {}
            }
        }
        true
    }

    /// From the home menu into the single-player settings.
    pub async fn single_player(&mut self) {
        self.wait_for("Single player").await;
        self.send(ENTER).await;
        self.wait_for("Settings").await;
    }

    pub async fn wait_for(&mut self, text: &str) {
        self.wait_until(text, Duration::from_secs(5), |p| p.text().contains(text))
            .await;
    }

    /// Waits for the server to close the channel; returns the exit status.
    pub async fn wait_closed(&mut self, limit: Duration) -> Option<u32> {
        self.wait_until("the connection to close", limit, |p| p.closed)
            .await;
        self.exit_status
    }
}

/// `GET path` from a server's admin port: the status code and the body.
/// The value of `name` in /metrics.
pub fn number(metrics: &str, name: &str) -> f64 {
    metrics
        .lines()
        .find_map(|l| l.strip_prefix(name)?.strip_prefix(' '))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no {name} in:\n{metrics}"))
}

pub async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: snaketui\r\n\r\n");
    conn.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    conn.read_to_string(&mut response).await.unwrap();
    let status = response[9..12].parse().unwrap();
    let body = response.split_once("\r\n\r\n").unwrap().1.to_string();
    (status, body)
}

pub const ENTER: &[u8] = b"\r";
pub const ESC: &[u8] = b"\x1b";
pub const CTRL_C: &[u8] = b"\x03";
