//! Passwords never reach the server's logs, even at debug level. This
//! file is its own test binary, so the log capture sees only this test.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;

const DOWN: &[u8] = b"\x1b[B";
const FIRST: &str = "Zebra-Quokka-913";
const WRONG: &str = "Wombat-Narwhal-27";
const SECOND: &str = "Ibex-Tapir-5150";

/// Everything the server logs, in one buffer.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn type_line(p: &Player, text: &str) {
    p.send(text.as_bytes()).await;
    p.send(ENTER).await;
}

#[tokio::test]
async fn passwords_never_reach_the_logs() {
    let logs = Capture::default();
    let writer = logs.clone();
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();
    let server = TestServer::start();

    // Sign up with a key.
    let key = new_key();
    let mut ann = Player::join(server.addr, Login::Key(&key), 100, 30).await;
    ann.wait_for("Quick play").await;
    for _ in 0..5 {
        ann.send(DOWN).await;
    }
    ann.send(ENTER).await;
    ann.wait_for("Sign up").await;
    type_line(&ann, "ann").await;
    type_line(&ann, FIRST).await;
    type_line(&ann, FIRST).await;
    ann.wait_for("Welcome, ann!").await;
    drop(ann);

    // Sign in without one, wrong and then right, and change the password.
    let mut p = Player::join(server.addr, Login::Guest, 100, 30).await;
    p.wait_for("Quick play").await;
    for _ in 0..6 {
        p.send(DOWN).await;
    }
    p.send(ENTER).await;
    p.wait_for("Sign in").await;
    type_line(&p, "ann").await;
    type_line(&p, WRONG).await;
    p.wait_for("Wrong username or password.").await;
    p.send(ENTER).await;
    type_line(&p, FIRST).await;
    p.wait_for("Signed in as ann").await;
    for _ in 0..4 {
        p.send(DOWN).await; // Quick play to Account
    }
    p.send(ENTER).await;
    p.wait_for("SHA256:").await;
    p.send(DOWN).await; // past ann's key
    p.send(ENTER).await;
    p.wait_for("Change password").await;
    type_line(&p, SECOND).await;
    type_line(&p, SECOND).await;
    p.wait_for("Password changed.").await;
    drop(p);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let logs = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
    // The capture works: sign-ins and the wrong try are there.
    for seen in ["signed up", "wrong sign-in", "signed in"] {
        assert!(logs.contains(seen), "no {seen:?} in the logs:\n{logs}");
    }
    for password in [FIRST, WRONG, SECOND] {
        // Whole, or any of its words. (The numbers turn up in timestamps.)
        let words = password
            .split('-')
            .filter(|w| w.chars().all(char::is_alphabetic));
        for piece in std::iter::once(password).chain(words) {
            assert!(
                !logs.contains(piece),
                "{piece:?} from a password is in the logs:\n{logs}"
            );
        }
    }
}
