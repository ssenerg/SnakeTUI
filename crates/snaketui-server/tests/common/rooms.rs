//! Private rooms through the home menu.

use std::net::SocketAddr;

use super::*;

const DOWN: &[u8] = b"\x1b[B";
const BACKSPACE: &[u8] = b"\x7f";

/// A guest called `~name` on the home menu.
pub async fn guest(addr: SocketAddr, name: &str, cols: u32, rows: u32) -> Player {
    let mut p = Player::join(addr, Login::GuestNamed(name), cols, rows).await;
    p.wait_for("Quick play").await;
    p
}

/// Presses Down `n` times on a fresh home menu, then Enter.
async fn home_pick(p: &Player, n: usize) {
    for _ in 0..n {
        p.send(DOWN).await;
    }
    p.send(ENTER).await;
}

/// Makes a private room from a fresh home menu and returns its code.
pub async fn create_room(p: &mut Player) -> String {
    home_pick(p, 2).await;
    p.wait_for("Private room").await;
    let text = p.text();
    let at = text.find("Code ").expect("a code on screen") + "Code ".len();
    text[at..at + 6].to_string()
}

pub async fn type_code(p: &Player, code: &str) {
    for _ in 0..6 {
        p.send(BACKSPACE).await;
    }
    p.send(code.as_bytes()).await;
}

/// Joins the room with `code` from a fresh home menu.
pub async fn join_room(p: &mut Player, code: &str) {
    home_pick(p, 3).await;
    p.wait_for("Join with a code").await;
    type_code(p, code).await;
    p.send(ENTER).await;
    p.wait_for("Private room").await;
}
