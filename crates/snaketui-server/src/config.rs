//! `server.toml`: where to listen, where to keep state, and limits.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Address players connect to. Port 22 lets them type plain `ssh host`;
    /// `[::]` takes both IPv6 and IPv4 players.
    pub listen: SocketAddr,
    /// The game's own SSH host key; created on first start. Never the
    /// machine's OpenSSH host key.
    pub host_key: PathBuf,
    /// `sqlite:///path/to/file.db`.
    pub database: String,
    /// Threads serving sessions; 0 means one per CPU core.
    pub workers: usize,
    /// Connections at once, in total and from one IP address.
    pub max_sessions: usize,
    pub max_sessions_per_ip: usize,
    /// Seconds a connection may take to sign in and open the game.
    pub login_timeout_secs: u64,
    /// Seconds without a key press, outside a running game, before a player
    /// is logged out.
    pub idle_timeout_secs: u64,
    /// Seconds a client may go without taking any screen updates before it's
    /// disconnected.
    pub stall_timeout_secs: u64,
    /// Tests only: every room's seed, so its boards are known in advance.
    #[serde(skip)]
    pub room_seed: Option<u64>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "[::]:22".parse().unwrap(),
            host_key: "/var/lib/snaketui/host_ed25519_key".into(),
            database: "sqlite:///var/lib/snaketui/snaketui.db".into(),
            workers: 0,
            max_sessions: 2000,
            max_sessions_per_ip: 10,
            login_timeout_secs: 30,
            idle_timeout_secs: 600,
            stall_timeout_secs: 20,
            room_seed: None,
        }
    }
}

impl ServerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn workers(&self) -> usize {
        if self.workers > 0 {
            self.workers
        } else {
            std::thread::available_parallelism().map_or(2, |n| n.get())
        }
    }

    pub fn login_timeout(&self) -> Duration {
        Duration::from_secs(self.login_timeout_secs)
    }

    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_secs)
    }

    pub fn stall_timeout(&self) -> Duration {
        Duration::from_secs(self.stall_timeout_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_config_parses_and_unknown_keys_are_errors() {
        let example = include_str!("../../../deploy/server.toml.example");
        let cfg: ServerConfig = toml::from_str(example).unwrap();
        assert_eq!(cfg.listen.port(), 22);
        assert!(toml::from_str::<ServerConfig>("listn = \"0.0.0.0:22\"").is_err());
    }
}
