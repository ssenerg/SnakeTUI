//! The quick-play queue for a cluster of `snaketui-server`s. Run one per
//! cluster, anywhere that reaches its NATS.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use snaketui_server::config::ServerConfig;
use snaketui_server::queue_service;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Matches quick-play players from every server of a cluster.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// The servers' settings file: its [cluster] table says which NATS and
    /// cluster to serve.
    #[arg(long, default_value = "/etc/snaketui/server.toml")]
    config: PathBuf,
    /// Tests only: runs the queue's clock this many times faster.
    #[arg(long, hide = true, default_value_t = 1.0)]
    speed: f64,
}

fn main() -> Result<()> {
    let logs = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_ansi(std::io::stdout().is_terminal());
    // The journal stamps every line itself.
    if std::env::var_os("JOURNAL_STREAM").is_some() {
        logs.without_time().init();
    } else {
        logs.init();
    }
    let args = Args::parse();
    let cfg = ServerConfig::load(&args.config)?;
    let cluster = cfg
        .cluster
        .with_context(|| format!("{} has no [cluster] table", args.config.display()))?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(queue_service::run(&cluster, args.speed, stop_signal()))
}

/// Ctrl+C, or SIGTERM from systemd.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
    info!("stop requested");
}
