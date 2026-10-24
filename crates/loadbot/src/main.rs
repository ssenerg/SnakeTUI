use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use snaketui_loadbot::{Settings, raise_open_files, start};

/// Plays SnakeTUI with many bots at once, to see how servers hold up.
/// Don't point it at servers you don't run.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Servers to connect to, as host:port; bots are spread over them.
    #[arg(required = true)]
    servers: Vec<String>,
    /// How many bots.
    #[arg(long, default_value_t = 100)]
    players: usize,
    /// Seconds over which the bots connect.
    #[arg(long, default_value_t = 30)]
    ramp: u64,
    /// Seconds to play for, after the ramp; 0 plays until Ctrl+C.
    #[arg(long = "for", default_value_t = 0)]
    duration: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let servers = args
        .servers
        .iter()
        .map(|s| {
            s.to_socket_addrs()
                .ok()
                .and_then(|mut a| a.next())
                .with_context(|| format!("can't find {s}"))
        })
        .collect::<Result<Vec<SocketAddr>>>()?;
    raise_open_files();
    tokio::runtime::Runtime::new()?.block_on(async {
        let stats = start(Settings {
            servers,
            players: args.players,
            ramp: Duration::from_secs(args.ramp),
        });
        let started = Instant::now();
        let end = (args.duration > 0).then(|| Duration::from_secs(args.ramp + args.duration));
        let mut every = tokio::time::interval(Duration::from_secs(5));
        let mut bytes = 0;
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                _ = every.tick() => {
                    let now = stats.bytes.load(Ordering::Relaxed);
                    println!(
                        "{:>4}s  {} playing  {} failures  {} KiB/s",
                        started.elapsed().as_secs(),
                        stats.playing.load(Ordering::Relaxed),
                        stats.failures.load(Ordering::Relaxed),
                        (now - bytes) / 1024 / 5
                    );
                    bytes = now;
                    if end.is_some_and(|end| started.elapsed() >= end) {
                        break;
                    }
                }
            }
        }
        Ok(())
    })
}
