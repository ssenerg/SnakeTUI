//! Postgres and NATS for SnakeTUI's tests.
//!
//! The first test that asks for one starts it in a Docker container, which
//! the rest of the test process shares. A watcher removes the container once
//! the process is gone, however it ends.
//!
//! To use servers you already run instead, set `SNAKETUI_TEST_POSTGRES` to
//! the URL of a database whose user may create databases, and
//! `SNAKETUI_TEST_NATS` to a NATS server with JetStream on.

use std::io::Read;
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use sqlx::{AssertSqlSafe, Connection, PgConnection};

const POSTGRES_IMAGE: &str = "postgres:18-alpine";
const NATS_IMAGE: &str = "nats:2.12-alpine";
const PASSWORD: &str = "snaketui";
/// How long a server may take to start.
const START_LIMIT: Duration = Duration::from_secs(60);

/// The URL of a new, empty database.
pub async fn postgres() -> String {
    static SERVER: OnceLock<String> = OnceLock::new();
    let server = SERVER.get_or_init(|| {
        std::env::var("SNAKETUI_TEST_POSTGRES").unwrap_or_else(|_| {
            let port = start(
                POSTGRES_IMAGE,
                5432,
                &[
                    "-e",
                    &format!("POSTGRES_PASSWORD={PASSWORD}"),
                    "--tmpfs",
                    "/var/lib/postgresql",
                ],
                // Nothing here needs to survive a crash, and tests open many
                // connections.
                &[
                    "postgres",
                    "-c",
                    "fsync=off",
                    "-c",
                    "synchronous_commit=off",
                    "-c",
                    "full_page_writes=off",
                    "-c",
                    "max_connections=1000",
                ],
            );
            format!("postgres://postgres:{PASSWORD}@127.0.0.1:{port}/postgres")
        })
    });

    static N: AtomicU32 = AtomicU32::new(0);
    let name = format!(
        "t_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let mut admin = connect_patiently(server).await;
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&mut admin)
        .await
        .expect("creating a test database");
    let _ = admin.close().await;
    with_database(server, &name)
}

/// The URL of a NATS server with JetStream, shared by the whole test process.
/// Tests keep apart by using names of their own.
pub fn nats() -> String {
    static SERVER: OnceLock<String> = OnceLock::new();
    SERVER
        .get_or_init(|| {
            std::env::var("SNAKETUI_TEST_NATS").unwrap_or_else(|_| {
                let port = start(NATS_IMAGE, 4222, &[], &["-js"]);
                wait_for_nats(port);
                format!("nats://127.0.0.1:{port}")
            })
        })
        .clone()
}

/// Starts `image` with `docker_args` and `command`, and returns the host port
/// its `port` is published on.
fn start(image: &str, port: u16, docker_args: &[&str], command: &[&str]) -> u16 {
    let published = format!("127.0.0.1::{port}");
    let out = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--label",
            "snaketui-test",
            "-p",
            &published,
        ])
        .args(docker_args)
        .arg(image)
        .args(command)
        .output()
        .expect("these tests need Docker, or SNAKETUI_TEST_POSTGRES and SNAKETUI_TEST_NATS");
    assert!(
        out.status.success(),
        "docker run {image}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8(out.stdout).unwrap().trim().to_string();
    remove_when_gone(&id);

    let out = Command::new("docker")
        .args(["port", &id, &format!("{port}/tcp")])
        .output()
        .unwrap();
    let mapping = String::from_utf8_lossy(&out.stdout);
    mapping
        .lines()
        .find_map(|line| line.rsplit_once(':')?.1.trim().parse().ok())
        .unwrap_or_else(|| panic!("no published port for {image}: {mapping:?}"))
}

/// Removes the container once this process has ended. The watcher ignores
/// Ctrl+C, which reaches it along with the tests.
// It's never waited for: it only ends after this process has.
#[allow(clippy::zombie_processes)]
fn remove_when_gone(id: &str) {
    let pid = std::process::id();
    let script = format!(
        "trap '' INT TERM HUP; while kill -0 {pid} 2>/dev/null; do sleep 1; done; \
         docker rm -f {id} >/dev/null 2>&1"
    );
    Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("starting the container's watcher");
}

/// Connects once the server takes connections. Postgres's image starts a
/// temporary server first that only listens on a socket, so a working TCP
/// connection means the real one is up.
async fn connect_patiently(url: &str) -> PgConnection {
    let deadline = Instant::now() + START_LIMIT;
    loop {
        match PgConnection::connect(url).await {
            Ok(conn) => return conn,
            Err(e) if Instant::now() > deadline => panic!("Postgres at {url} never came up: {e}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

/// Waits until NATS greets a connection. Docker's port proxy takes
/// connections before the server does, so connecting alone proves nothing.
fn wait_for_nats(port: u16) {
    let deadline = Instant::now() + START_LIMIT;
    while Instant::now() < deadline {
        if let Ok(mut conn) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = conn.set_read_timeout(Some(Duration::from_secs(1)));
            let mut greeting = [0; 4];
            if conn.read_exact(&mut greeting).is_ok() && &greeting == b"INFO" {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("NATS on port {port} never came up");
}

/// `url` with its database swapped for `name`.
fn with_database(url: &str, name: &str) -> String {
    let (base, query) = url
        .split_once('?')
        .map_or((url, None), |(b, q)| (b, Some(q)));
    let (server, _) = base.rsplit_once('/').expect("a URL with a database path");
    match query {
        Some(query) => format!("{server}/{name}?{query}"),
        None => format!("{server}/{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_urls_keep_their_options() {
        assert_eq!(
            with_database("postgres://u:p@h:5432/postgres", "t_1"),
            "postgres://u:p@h:5432/t_1"
        );
        assert_eq!(
            with_database("postgres://u@h/db?sslmode=disable", "t_2"),
            "postgres://u@h/t_2?sslmode=disable"
        );
    }

    #[tokio::test]
    async fn each_database_is_new() {
        let a = postgres().await;
        let b = postgres().await;
        assert_ne!(a, b);
        let mut conn = PgConnection::connect(&a).await.unwrap();
        sqlx::raw_sql("CREATE TABLE t (x INT)")
            .execute(&mut conn)
            .await
            .unwrap();
    }
}
