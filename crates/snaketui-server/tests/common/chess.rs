//! Chess through the menus: challenges in private rooms, and moves.

use std::net::SocketAddr;

use russh::keys::PrivateKey;

use super::accounts::{DOWN, UP, connect_as, sign_up};
use super::rooms::{create_room, join_room};
use super::*;

pub const LEFT: &[u8] = b"\x1b[D";
pub const RIGHT: &[u8] = b"\x1b[C";

/// A guest who typed `ssh chess@host`, on chess's home menu.
pub async fn guest(addr: SocketAddr) -> Player {
    let mut p = Player::join(addr, Login::GuestNamed("chess"), 100, 30).await;
    p.wait_for("Board and pieces").await;
    p
}

/// A new account called `name`, opening chess with its own key, at the
/// top of the home menu.
pub async fn account(addr: SocketAddr, name: &str) -> (Player, PrivateKey) {
    let key = new_key();
    let mut p = connect_as(addr, Login::KeyAs(&key, "chess")).await;
    p.wait_for("Board and pieces").await;
    sign_up(&mut p, name).await;
    p.send(UP).await;
    (p, key)
}

/// Back with `key` after a dropped connection, in the game still going.
pub async fn back(addr: SocketAddr, key: &PrivateKey) -> Player {
    let mut p = Player::join(addr, Login::KeyAs(key, "chess"), 100, 30).await;
    p.wait_for("Welcome back: your game went on.").await;
    p
}

/// `ann`, white, and `ben` play two moves; `ann`'s connection drops, and
/// `ben` sees her seat waiting.
pub async fn two_moves_then_ann_drops(mut ann: Player, ben: &mut Player) {
    start(&mut ann, ben).await;
    play(&mut ann, "", "e4").await;
    play(ben, "1. e4", "e5").await;
    ann.wait_for("1. e4      e5").await;
    drop(ann);
    ben.wait_for("ann (away)").await;
    ben.wait_for("is away · back within 1:").await;
}

/// `ann` makes a room where she plays white, `keys` set its time control
/// on the challenge screen, and `ben` joins it. Both start from chess's
/// home menu.
pub async fn challenge(ann: &mut Player, ben: &mut Player, keys: &[&[u8]]) {
    let code = create_room(ann).await;
    ann.send(b"e").await;
    ann.wait_for("Challenge").await;
    for &key in keys {
        ann.send(key).await;
    }
    // The host's side: random, then white.
    ann.send(DOWN).await;
    ann.send(DOWN).await;
    ann.send(RIGHT).await;
    ann.wait_for("◂ white ▸").await;
    ann.send(ESC).await;
    ann.wait_for("you play white").await;
    join_room(ben, &code).await;
    ben.wait_for("plays white").await;
}

/// Both say they're ready, and the game starts.
pub async fn start(ann: &mut Player, ben: &mut Player) {
    ann.send(ENTER).await;
    ben.send(ENTER).await;
    ann.wait_for("Your move within").await;
    ben.wait_for("White to move within").await;
}

/// `who` plays `san` once the moves so far end with `after`, as the
/// panel lists them.
pub async fn play(who: &mut Player, after: &str, san: &str) {
    if !after.is_empty() {
        who.wait_for(after).await;
    }
    who.wait_for("Your move").await;
    who.send(san.as_bytes()).await;
    who.send(ENTER).await;
}
