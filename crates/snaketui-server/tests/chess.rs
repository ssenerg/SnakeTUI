//! Chess, end to end: a challenge in a private room, played to mate, on
//! time and to a draw, with a rematch; quick pairing, rated; and friends
//! watching.

mod common;

use std::time::Duration;

use common::accounts::line_of;
use common::accounts::{DOWN, UP, connect_as, make_friends, sign_up};
use common::chess::{LEFT, RIGHT, account, back, challenge, play, start, two_moves_then_ann_drops};
use common::rooms::{create_room, join_room};
use common::*;

fn chess_server() -> TestServer {
    TestServer::with(|cfg| cfg.enabled_games = vec!["snake".into(), "chess".into()])
}

/// A chess guest on `server`.
async fn guest(server: &TestServer) -> Player {
    common::chess::guest(server.addr).await
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

/// On quick pairing's screen, `keys` move to `pool` and wait in it.
async fn choose_pool(p: &mut Player, keys: &[&[u8]], pool: &str) {
    p.wait_for("Quick pairing").await;
    for &key in keys {
        p.send(key).await;
    }
    p.wait_for(&format!("{pool} · sides drawn · rated")).await;
    p.send(ENTER).await;
    p.wait_for("Finding a match").await;
    p.wait_for(pool).await;
}

#[tokio::test]
async fn quick_pairing_keeps_presets_apart_and_rates_each_category_alone() {
    let server = chess_server();
    let (mut ann, _) = account(server.addr, "ann").await;
    let (mut ben, _) = account(server.addr, "ben").await;
    // From the top of the home menu to quick play. Pairing starts on 5+3:
    // up and left is bullet's 1+0, down and left twice rapid's 10+0.
    for p in [&mut ann, &mut ben] {
        p.send(DOWN).await;
        p.send(ENTER).await;
    }
    choose_pool(&mut ann, &[UP, LEFT], "1+0 bullet").await;
    choose_pool(&mut ben, &[DOWN, LEFT, LEFT], "10+0 rapid").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    for p in [&ann, &ben] {
        let text = p.text();
        assert!(text.contains("Finding a match"), "{text}");
        assert!(text.contains("1 in queue"), "each alone in theirs: {text}");
    }

    // Ben goes to bullet instead, from 10+0 where pairing opens now.
    ben.send(ESC).await;
    ben.wait_until("the home menu", Duration::from_secs(5), |p| {
        !p.text().contains("Finding a match")
    })
    .await;
    ben.send(ENTER).await;
    choose_pool(&mut ben, &[UP, UP], "1+0 bullet").await;
    for p in [&mut ann, &mut ben] {
        p.wait_for("move within").await;
    }
    let (mut white, mut black) = if ann.text().contains("Your move within") {
        (ann, ben)
    } else {
        (ben, ann)
    };
    play(&mut white, "", "e4").await;
    play(&mut black, "1. e4", "e5").await;
    white.wait_for("1. e4      e5").await;
    black.send(ESC).await;
    black.wait_for("Resign").await;
    black.send(DOWN).await;
    black.send(ENTER).await;
    black.wait_for("Resign this game?").await;
    black.send(b"y").await;
    white.wait_for("You win!").await;
    // Both new, both very unsure: Glicko-2 moves them far.
    white.wait_for("+162").await;
    black.wait_for("−162").await;

    // Playing again shows bullet's new rating, and rapid's untouched.
    white.send(ENTER).await;
    white
        .wait_until("bullet's new rating", Duration::from_secs(5), |p| {
            line_of(&p.text(), "Bullet").contains("1662?")
        })
        .await;
    let text = white.text();
    assert!(line_of(&text, "Rapid").contains("1500?"), "{text}");
    assert!(text.contains("1+0 bullet · sides drawn · rated"), "{text}");
}

#[tokio::test]
async fn a_dropped_player_comes_back_to_their_game() {
    let server = chess_server();
    let (mut ann, key) = account(server.addr, "ann").await;
    let (mut ben, _) = account(server.addr, "ben").await;
    challenge(&mut ann, &mut ben, &[]).await;
    two_moves_then_ann_drops(ann, &mut ben).await;

    // Her key signs her in, and she's back where she was, with her clock
    // having run while she was gone.
    let mut ann = back(server.addr, &key).await;
    ann.wait_for("1. e4      e5").await;
    ann.wait_until("her clock to have run", Duration::from_secs(5), |p| {
        line_of(&p.text(), "ann (you)").contains("9:5")
    })
    .await;
    play(&mut ann, "", "Nf3").await;
    ben.wait_for("2. Nf3").await;
    ben.wait_until("ann to be back", Duration::from_secs(5), |p| {
        !p.text().contains("away")
    })
    .await;
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
