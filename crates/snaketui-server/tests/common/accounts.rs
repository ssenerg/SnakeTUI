//! Signing up and in through the home menu.

use std::net::SocketAddr;
use std::time::Duration;

use russh::keys::PrivateKey;

use super::*;

pub const UP: &[u8] = b"\x1b[A";
pub const DOWN: &[u8] = b"\x1b[B";
pub const BACKSPACE: &[u8] = b"\x7f";
pub const PASSWORD: &str = "correct horse 7";
pub const WRONG: &str = "Wrong username or password.";

// A guest's home menu: Single player, Quick play, Create, Join, Nickname,
// Sign up, Sign in, Quit. An account's: ..., Join, Friends, Account, Quit.
// Signing up or in leaves the cursor on Quick play.
pub const QUICK_PLAY: usize = 1;
pub const SIGN_UP: usize = 5;
pub const SIGN_IN: usize = 6;
pub const FRIENDS: usize = 4;
pub const ACCOUNT: usize = 5;

/// The home menu, guest or signed in, waiting for it to show.
pub async fn connect_as(addr: SocketAddr, login: Login<'_>) -> Player {
    let mut p = Player::join(addr, login, 100, 30).await;
    p.wait_for("Quick play").await;
    p
}

/// Presses Down `n` times, then Enter.
pub async fn down_enter(p: &Player, n: usize) {
    for _ in 0..n {
        p.send(DOWN).await;
    }
    p.send(ENTER).await;
}

/// Moves the home menu's cursor from `from` down to `to` and picks it.
pub async fn pick(p: &Player, from: usize, to: usize) {
    down_enter(p, to - from).await;
}

/// Signs up from a fresh home menu.
pub async fn sign_up(p: &mut Player, name: &str) {
    pick(p, 0, SIGN_UP).await;
    p.wait_for("Sign up").await;
    p.send(name.as_bytes()).await;
    p.send(ENTER).await;
    p.send(PASSWORD.as_bytes()).await;
    p.send(ENTER).await;
    p.send(PASSWORD.as_bytes()).await;
    p.send(ENTER).await;
    p.wait_for(&format!("Welcome, {name}!")).await;
}

/// Fills in the sign-in form from a fresh home menu. With a key the form
/// ends with "remember this machine", left ticked.
pub async fn try_sign_in(p: &mut Player, name: &str, password: &str, with_key: bool) {
    pick(p, 0, SIGN_IN).await;
    p.wait_for("Sign in").await;
    p.send(name.as_bytes()).await;
    p.send(ENTER).await;
    p.send(password.as_bytes()).await;
    p.send(ENTER).await;
    if with_key {
        p.send(ENTER).await;
    }
}

/// After a refused sign-in, with the cursor back on the username: types
/// a new name and password and sends them.
pub async fn retry_sign_in(p: &mut Player, name: &str, password: &str) {
    for _ in 0..16 {
        p.send(BACKSPACE).await;
    }
    p.send(name.as_bytes()).await;
    // Typing clears the last error, so the next one is new.
    p.wait_until("the error to clear", Duration::from_secs(5), |p| {
        !p.text().contains(WRONG)
    })
    .await;
    p.send(ENTER).await;
    p.send(password.as_bytes()).await;
    p.send(ENTER).await;
}

pub async fn signed_up(addr: SocketAddr, name: &str) -> (Player, PrivateKey) {
    let key = new_key();
    let mut p = connect_as(addr, Login::Key(&key)).await;
    sign_up(&mut p, name).await;
    (p, key)
}

/// `ann` asks `ben` to be friends and `ben` accepts; both end on their
/// Friends screens with the other one listed.
pub async fn make_friends(ann: &mut Player, ben: &mut Player) {
    pick(ann, QUICK_PLAY, FRIENDS).await;
    ann.wait_for("Add a friend").await;
    ann.send(ENTER).await;
    ann.wait_for("Their username").await;
    ann.send(b"ben").await;
    ann.send(ENTER).await;
    ann.wait_for("Asked ben to be friends.").await;

    ben.wait_for("ann wants to be friends").await;
    ben.wait_for("1 request").await;
    pick(ben, QUICK_PLAY, FRIENDS).await;
    ben.wait_for("a accept").await;
    ben.send(DOWN).await;
    ben.send(b"a").await;
    ben.wait_for("You and ann are now friends.").await;
    ann.wait_for("You and ben are now friends.").await;
}

/// The line of `text` that names `who`.
pub fn line_of<'a>(text: &'a str, who: &str) -> &'a str {
    text.lines().find(|l| l.contains(who)).unwrap_or("")
}
