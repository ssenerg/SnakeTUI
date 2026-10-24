//! A real `snaketui-server` process, for tests that kill servers outright.

use std::fs::File;
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub struct ServerProcess {
    pub addr: SocketAddr,
    pub dir: PathBuf,
    child: Option<Child>,
}

impl ServerProcess {
    /// Starts a server whose `server.toml` is a port of its choosing, its
    /// own host key and two workers, then `settings`. Wait for it with
    /// [`Self::ready`], which learns the port.
    pub fn spawn(settings: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "snaketui-proc-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("server.toml");
        std::fs::write(
            &config,
            format!(
                "listen = \"127.0.0.1:0\"\nhost_key = \"{}\"\nworkers = 2\n{settings}",
                dir.join("host_key").display()
            ),
        )
        .unwrap();
        let log = File::create(dir.join("log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_snaketui-server"))
            .arg("--config")
            .arg(&config)
            .env("RUST_LOG", "snaketui_server=debug")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("starting snaketui-server");
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            dir,
            child: Some(child),
        }
    }

    /// Waits until the server says where it listens, and greets
    /// connections there.
    pub fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Some(Ok(Some(status))) = self.child.as_mut().map(Child::try_wait) {
                panic!("the server exited ({status}):\n{}", self.log());
            }
            if self.addr.port() == 0 {
                if let Some(addr) = listening_on(&self.log()) {
                    self.addr = addr;
                }
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            if let Ok(mut conn) = TcpStream::connect(self.addr) {
                let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                let mut banner = [0; 4];
                if conn.read_exact(&mut banner).is_ok() && &banner == b"SSH-" {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the server never came up:\n{}", self.log());
    }

    /// Kills the server with no chance to say goodbye, as a crash would.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("log")).unwrap_or_default()
    }
}

/// The address in the server's "serving SnakeTUI over SSH addr=..." line.
fn listening_on(log: &str) -> Option<SocketAddr> {
    let line = log
        .lines()
        .find(|l| l.contains("serving SnakeTUI over SSH"))?;
    let at = line.find("addr=")? + "addr=".len();
    line[at..].split_whitespace().next()?.parse().ok()
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A `snaketui-matchmaker` process: the cluster's quick-play queue.
pub struct MatchmakerProcess {
    pub dir: PathBuf,
    child: Option<Child>,
}

impl MatchmakerProcess {
    /// Starts a queue for the cluster in `settings` (a server.toml), with
    /// its clock running `speed` times faster than real time.
    pub fn start(settings: &str, speed: f64) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "snaketui-queue-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("server.toml");
        std::fs::write(&config, settings).unwrap();
        let log = File::create(dir.join("log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_snaketui-matchmaker"))
            .arg("--config")
            .arg(&config)
            .arg("--speed")
            .arg(speed.to_string())
            .env("RUST_LOG", "snaketui_server=debug")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("starting snaketui-matchmaker");
        let mut queue = Self {
            dir,
            child: Some(child),
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        while !queue.log().contains("matchmaking for the cluster") {
            if let Some(Ok(Some(status))) = queue.child.as_mut().map(Child::try_wait) {
                panic!("the matchmaker exited ({status}):\n{}", queue.log());
            }
            assert!(
                Instant::now() < deadline,
                "the matchmaker never came up:\n{}",
                queue.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        queue
    }

    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("log")).unwrap_or_default()
    }
}

impl Drop for MatchmakerProcess {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
