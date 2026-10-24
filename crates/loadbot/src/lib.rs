//! Bot players for SnakeTUI servers. Each one signs in as a guest, joins
//! quick play and steers at random, now and then pressing Enter, which
//! queues it again after a match. Bots that lose their connection come
//! back a second later.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::PublicKeyOrCertificate;
use russh::{ChannelMsg, ChannelWriteHalf};

const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const LEFT: &[u8] = b"\x1b[D";
const RIGHT: &[u8] = b"\x1b[C";
const ENTER: &[u8] = b"\r";
/// How often a bot presses a key.
const KEY_EVERY: Duration = Duration::from_millis(150);

pub struct Settings {
    /// Bots are spread over these in turn.
    pub servers: Vec<SocketAddr>,
    pub players: usize,
    /// Bots connect evenly over this long.
    pub ramp: Duration,
}

/// What the bots have seen so far.
#[derive(Default)]
pub struct Stats {
    /// Bots signed in and playing now.
    pub playing: AtomicUsize,
    /// Connections that failed, or were dropped.
    pub failures: AtomicU64,
    /// Bytes of screen received.
    pub bytes: AtomicU64,
}

/// Starts the bots; they play until the runtime stops. The stats are
/// updated as they go.
pub fn start(settings: Settings) -> Arc<Stats> {
    let stats = Arc::new(Stats::default());
    let gap = settings.ramp / settings.players.max(1) as u32;
    let servers = settings.servers.clone();
    let players = settings.players;
    let all = Arc::clone(&stats);
    tokio::spawn(async move {
        for i in 0..players {
            let server = servers[i % servers.len()];
            tokio::spawn(bot(i, server, Arc::clone(&all)));
            tokio::time::sleep(gap).await;
        }
    });
    stats
}

struct AnyHost;

impl client::Handler for AnyHost {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// One bot, for good.
async fn bot(i: usize, server: SocketAddr, stats: Arc<Stats>) {
    let mut seed = i as u64 + 1;
    loop {
        if play(i, server, &stats, &mut seed).await.is_err() {
            stats.failures.fetch_add(1, Ordering::Relaxed);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Plays until the connection ends.
async fn play(i: usize, server: SocketAddr, stats: &Stats, seed: &mut u64) -> anyhow::Result<()> {
    let config = Arc::new(client::Config::default());
    let mut ssh = client::connect(config, server, AnyHost).await?;
    let signed_in = ssh
        .authenticate_keyboard_interactive_start(format!("bot{i}"), None)
        .await?;
    anyhow::ensure!(
        matches!(signed_in, KeyboardInteractiveAuthResponse::Success),
        "not let in"
    );
    let channel = ssh.channel_open_session().await?;
    channel
        .request_pty(true, "xterm-256color", 80, 24, 0, 0, &[])
        .await?;
    channel.request_shell(true).await?;
    let (mut rx, tx) = channel.split();
    stats.playing.fetch_add(1, Ordering::Relaxed);
    let reading = async {
        while let Some(msg) = rx.wait().await {
            if let ChannelMsg::Data { data } = msg {
                stats.bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
            }
        }
    };
    let ended = tokio::select! {
        _ = reading => Ok(()),
        pressed = press_keys(&tx, seed) => pressed,
    };
    stats.playing.fetch_sub(1, Ordering::Relaxed);
    ended
}

/// Into quick play from the home menu, then mostly steering; now and then
/// Enter, which queues again after a match. Ends only if sending fails.
async fn press_keys(tx: &ChannelWriteHalf<client::Msg>, seed: &mut u64) -> anyhow::Result<()> {
    tx.data_bytes(DOWN.to_vec()).await?;
    tx.data_bytes(ENTER.to_vec()).await?;
    let keys = [UP, LEFT, DOWN, RIGHT];
    loop {
        tokio::time::sleep(KEY_EVERY).await;
        *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let n = (*seed >> 33) as usize;
        let key = if n.is_multiple_of(13) {
            ENTER
        } else {
            keys[n % 4]
        };
        tx.data_bytes(key.to_vec()).await?;
    }
}

/// Raises this process's limit on open files as far as it goes, so
/// thousands of bots fit.
pub fn raise_open_files() {
    #[cfg(unix)]
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return;
        }
        for want in [1 << 20, 1 << 16, 1 << 14] {
            let raised = libc::rlimit {
                rlim_cur: limit.rlim_max.min(want),
                rlim_max: limit.rlim_max,
            };
            if raised.rlim_cur <= limit.rlim_cur
                || libc::setrlimit(libc::RLIMIT_NOFILE, &raised) == 0
            {
                return;
            }
        }
    }
}
