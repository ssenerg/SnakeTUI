//! Chess, end to end: a challenge in a private room, played to mate, on
//! time and to a draw, with a rematch; and friends watching.

mod common;

use std::time::Duration;

use common::accounts::{DOWN, UP, connect_as, make_friends, sign_up};
use common::rooms::{create_room, join_room};
use common::*;

const LEFT: &[u8] = b"\x1b[D";
const RIGHT: &[u8] = b"\x1b[C";

fn chess_server() -> TestServer {
    TestServer::with(|cfg| cfg.enabled_games = vec!["snake".into(), "chess".into()])
}

/// A guest who typed `ssh chess@host`, on chess's home menu.
async fn guest(server: &TestServer) -> Player {
    let mut p = Player::join(server.addr, Login::GuestNamed("chess"), 100, 30).await;
    p.wait_for("Board and pieces").await;
    p
}

/// `ann` makes a room where she plays white, `keys` set its time control,
/// and `ben` joins it.
async fn challenge(ann: &mut Player, ben: &mut Player, keys: &[&[u8]]) {
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
async fn start(ann: &mut Player, ben: &mut Player) {
    ann.send(ENTER).await;
    ben.send(ENTER).await;
    ann.wait_for("Your move within").await;
    ben.wait_for("White to move within").await;
}

/// `who` plays `san` once the moves so far end with `after`, as the
/// panel lists them.
async fn play(who: &mut Player, after: &str, san: &str) {
    if !after.is_empty() {
        who.wait_for(after).await;
    }
    who.wait_for("Your move").await;
    who.send(san.as_bytes()).await;
    who.send(ENTER).await;
}

#[tokio::test]
async fn ssh_chess_opens_chess_once_it_is_served() {
    let server = chess_server();
    let p = guest(&server).await;
    let home = p.text();
    assert!(home.contains("Create a private room  10+0 rapid"), "{home}");
    // Two games: plain `ssh` lets a new player choose.
    let mut q = Player::join(server.addr, Login::Guest, 100, 30).await;
    q.wait_for("Chess").await;
    assert!(q.text().contains("Snake"), "{}", q.text());
}

#[tokio::test]
async fn a_one_minute_challenge_is_played_to_mate() {
    let server = chess_server();
    let (mut ann, mut ben) = (guest(&server).await, guest(&server).await);
    // From 10+0 down the presets to 1+0, then the side.
    let code = create_room(&mut ann).await;
    for _ in 0..6 {
        ann.send(LEFT).await;
    }
    ann.wait_for("1+0 bullet").await;
    ann.send(b"e").await;
    ann.wait_for("Challenge").await;
    ann.send(DOWN).await;
    ann.send(DOWN).await;
    ann.send(RIGHT).await;
    ann.send(ESC).await;
    ann.wait_for("you play white").await;
    join_room(&mut ben, &code).await;
    ben.wait_for("1+0 bullet").await;
    start(&mut ann, &mut ben).await;

    // Fool's mate, typed.
    play(&mut ann, "", "f3").await;
    play(&mut ben, "1. f3", "e5").await;
    play(&mut ann, "1. f3      e5", "g4").await;
    play(&mut ben, "2. g4", "Qh4").await;
    ben.wait_for("You win!").await;
    ann.wait_for("wins!").await;
    ann.wait_for("Black won by checkmate").await;
}

#[tokio::test]
async fn the_clock_runs_out() {
    let server = chess_server();
    let (mut ann, mut ben) = (guest(&server).await, guest(&server).await);
    // Minutes from 10 down to a quarter.
    challenge(&mut ann, &mut ben, &[LEFT; 13]).await;
    ben.wait_for("0.25+0 bullet").await;
    start(&mut ann, &mut ben).await;
    play(&mut ann, "", "e4").await;
    play(&mut ben, "1. e4", "e5").await;
    // White's clock runs, and white never moves again.
    ann.wait_for("1. e4      e5").await;
    ann.wait_until("white's flag to fall", Duration::from_secs(25), |p| {
        p.text().contains("Black won by time")
    })
    .await;
    ben.wait_for("You win!").await;
}

#[tokio::test]
async fn a_draw_is_agreed_and_a_rematch_follows() {
    let server = chess_server();
    let (mut ann, mut ben) = (guest(&server).await, guest(&server).await);
    challenge(&mut ann, &mut ben, &[]).await;
    start(&mut ann, &mut ben).await;
    play(&mut ann, "", "e4").await;
    play(&mut ben, "1. e4", "e5").await;
    ann.wait_for("1. e4      e5").await;
    // The menu's first entry offers a draw, and the other side's accepts.
    ann.send(ESC).await;
    ann.wait_for("Offer a draw").await;
    ann.send(ENTER).await;
    ann.wait_for("You offered a draw").await;
    ben.wait_for("offers a draw").await;
    ben.send(ESC).await;
    ben.wait_for("Accept the draw").await;
    ben.send(ENTER).await;
    for p in [&mut ann, &mut ben] {
        p.wait_for("Draw!").await;
        p.wait_for("Drawn by agreement").await;
    }
    // Back in the lobby, nothing starts until both are ready again.
    ann.wait_until("the lobby", Duration::from_secs(20), |p| {
        p.text().contains("Starting when everyone is ready")
    })
    .await;
    start(&mut ann, &mut ben).await;
}

#[tokio::test]
async fn friends_watch_a_game() {
    let server = chess_server();
    let mut accounts = Vec::new();
    for name in ["ann", "ben"] {
        let key = new_key();
        let mut p = connect_as(server.addr, Login::KeyAs(&key, "chess")).await;
        p.wait_for("Board and pieces").await;
        sign_up(&mut p, name).await;
        accounts.push(p);
    }
    let [mut ann, mut ben]: [Player; 2] = accounts.try_into().ok().unwrap();
    make_friends(&mut ann, &mut ben).await;

    // ann makes a room, which carl joins.
    ann.send(ESC).await;
    ann.wait_for("Signed in as ann").await;
    ann.send(UP).await;
    ann.send(UP).await;
    ann.send(ENTER).await;
    ann.wait_for("Private room").await;
    let code = {
        let text = ann.text();
        let at = text.find("Code ").expect("a code") + "Code ".len();
        text[at..at + 6].to_string()
    };
    let mut carl = guest(&server).await;
    join_room(&mut carl, &code).await;

    // ben finds ann in his friends list, and watches.
    ben.send(ESC).await;
    ben.wait_for("Signed in as ben").await;
    ben.send(ENTER).await;
    ben.wait_for("in a private room · v watch").await;
    ben.send(DOWN).await;
    ben.send(b"v").await;
    ben.wait_for("Watching ann").await;
    ann.wait_for("1 watching").await;

    // The game starts, and its first move shows for him too. Either of
    // them may be white.
    ann.send(ENTER).await;
    carl.send(ENTER).await;
    ann.wait_for("move within").await;
    carl.wait_for("move within").await;
    ann.send(b"e4\r").await;
    carl.send(b"e4\r").await;
    ben.wait_for("1. e4").await;
    assert!(
        ben.text().contains("Watching ann · f flip"),
        "{}",
        ben.text()
    );
}
