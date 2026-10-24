use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use russh::keys::HashAlg;
use snaketui_server::config::ServerConfig;
use snaketui_server::{serve, ssh};
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Serves SnakeTUI to anyone who connects with `ssh`.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Settings file. Without one, /etc/snaketui/server.toml is used if it
    /// exists, otherwise the defaults.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Listen here instead of the configured address.
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// Create the configured host key if it's missing, print its
    /// fingerprint, and exit. Servers in a cluster share one key: make it
    /// once and copy it to the others.
    #[arg(long)]
    make_host_key: bool,
}

const DEFAULT_CONFIG: &str = "/etc/snaketui/server.toml";

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
    let mut cfg = match args.config {
        Some(path) => ServerConfig::load(&path)?,
        None if std::path::Path::new(DEFAULT_CONFIG).exists() => {
            ServerConfig::load(DEFAULT_CONFIG.as_ref())?
        }
        None => ServerConfig::default(),
    };
    if let Some(listen) = args.listen {
        cfg.listen = listen;
    }
    let host_key = ssh::load_or_create_host_key(&cfg.host_key)?;
    if args.make_host_key {
        let public = host_key.public_key();
        println!(
            "{} {}",
            public.fingerprint(HashAlg::Sha256),
            cfg.host_key.display()
        );
        return Ok(());
    }
    // Bind before starting anything, so a taken port fails right away.
    let listener = std::net::TcpListener::bind(cfg.listen)
        .with_context(|| format!("listening on {}", cfg.listen))?;
    let admin = cfg
        .admin_listen
        .map(|addr| {
            std::net::TcpListener::bind(addr).with_context(|| format!("listening on {addr}"))
        })
        .transpose()?;
    actix_rt::System::new().block_on(serve(cfg, listener, admin, host_key, stop_signal()))
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
