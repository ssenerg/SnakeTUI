//! Multiplayer end to end: several SSH clients on one real server, in quick
//! play and in private rooms.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use common::rooms::*;
use common::*;
use russh::ChannelMsg;
use snake_core::config::Rules;
use snake_core::round::{Entrant, Room, RoomKind, RoomSettings};
use snaketui_server::metrics::TICK_LAG;
use tokio::time::{Instant, sleep};

const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const LEFT: &[u8] = b"\x1b[D";
const RIGHT: &[u8] = b"\x1b[C";
const BACKSPACE: &[u8] = b"\x7f";

/// A match fills for 10 s before starting with two players.
const MATCH_WAIT: Duration = Duration::from_secs(20);

/// Presses Down `n` times on the home menu, then Enter.
async fn pick(p: &Player, n: usize) {
    for _ in 0..n {
        p.send(DOWN).await;
    }
    p.send(ENTER).await;
}

async fn quick_play(p: &mut Player) {
    pick(p, 1).await;
    p.wait_for("Finding a match").await;
}

/// The first seed whose first round, played with nobody pressing a key,
/// ends the way `wanted` says, given its winner (`None` for a draw), for
/// `players` in a private room of `rules`.
fn seed_where(players: u64, rules: &Rules, wanted: impl Fn(Option<&Entrant>, u64) -> bool) -> u64 {
    let settings = RoomSettings::new(rules.clone(), RoomKind::Private).unwrap();
    let tick = Duration::from_millis(settings.game.tick_ms);
    (0..10_000)
        .find(|&seed| {
            let mut round = Room::new(settings.clone(), seed);
            for id in 1..=players {
                round
                    .join(Entrant {
                        id,
                        name: id.to_string(),
                    })
                    .unwrap();
                round.toggle_ready(id);
            }
            let started = round.view();
            assert!(
                started.game.board.is_some(),
                "everyone ready starts the countdown"
            );
            for step in 0..2_000u64 {
                if let Some(end) = round.advance(tick) {
                    return wanted(game_api::winner(end.placements()), step);
                }
            }
            false
        })
        .expect("some seed works")
}

#[tokio::test]
async fn two_players_are_matched_and_see_the_countdown_and_each_other() {
    let server = TestServer::start();
    let mut ann = guest(server.addr, "ann", 80, 24).await;
    let mut ben = guest(server.addr, "ben", 80, 24).await;
    quick_play(&mut ann).await;
    quick_play(&mut ben).await;
    for p in [&mut ann, &mut ben] {
        p.wait_until("the countdown", MATCH_WAIT, |p| {
            p.text().contains("GET READY")
        })
        .await;
        let text = p.text();
        assert!(text.contains("~ann") && text.contains("~ben"), "{text}");
    }
    ann.wait_for("←↑↓→ move").await;
}

#[tokio::test]
async fn a_round_where_everyone_crashes_at_once_is_a_draw_and_rooms_play_on() {
    let seed = seed_where(2, &Rules::default(), |winner, _| winner.is_none());
    let server = TestServer::with(|c| c.room_seed = Some(seed));
    let mut ann = guest(server.addr, "ann", 100, 30).await;
    let mut ben = guest(server.addr, "ben", 100, 30).await;
    let code = create_room(&mut ann).await;
    join_room(&mut ben, &code).await;
    ann.wait_for("~ben").await;
    ann.wait_for("Starting in").await;
    for p in [&ann, &ben] {
        p.send(ENTER).await;
    }
    for p in [&mut ann, &mut ben] {
        p.wait_for("GET READY").await;
        p.wait_until("the draw", Duration::from_secs(15), |p| {
            p.text().contains("Draw!")
        })
        .await;
        p.wait_for("Next round in").await;
    }
    // After the results, the same room gathers for another round.
    for p in [&mut ann, &mut ben] {
        p.wait_until("the lobby", Duration::from_secs(12), |p| {
            p.text().contains("Private room")
        })
        .await;
        p.send(ENTER).await;
    }
    for p in [&mut ann, &mut ben] {
        p.wait_for("GET READY").await;
    }
}

#[tokio::test]
async fn leaving_and_dropping_out_mid_round_leave_the_others_playing() {
    // Nobody crashes in the first two seconds without steering.
    let seed = seed_where(3, &Rules::default(), |_, step| step > 14);
    let server = TestServer::with(|c| c.room_seed = Some(seed));
    let mut ann = guest(server.addr, "ann", 100, 30).await;
    let mut ben = guest(server.addr, "ben", 100, 30).await;
    let mut cat = guest(server.addr, "cat", 100, 30).await;
    let code = create_room(&mut ann).await;
    join_room(&mut ben, &code).await;
    join_room(&mut cat, &code).await;
    ann.wait_for("~cat").await;
    for p in [&ann, &ben, &cat] {
        p.send(ENTER).await;
    }
    for p in [&mut ann, &mut ben, &mut cat] {
        p.wait_for("←↑↓→ move").await;
    }

    // Esc asks first; y leaves while the round goes on for the others.
    ann.send(ESC).await;
    ann.wait_for("Leave the match?").await;
    ann.send(b"y").await;
    ann.wait_for("Quick play").await;
    ben.wait_until("ann's snake gone", Duration::from_secs(3), |p| {
        p.text()
            .lines()
            .any(|l| l.contains("✖") && l.contains("~ann"))
    })
    .await;
    assert!(
        ben.text().contains("←↑↓→ move"),
        "still playing:\n{}",
        ben.text()
    );

    // A dropped connection counts as dying too, which leaves ben the winner.
    drop(cat);
    ben.wait_for("You win!").await;
}

#[tokio::test]
async fn nine_players_in_the_queue_make_one_full_match_and_one_waits() {
    let server = TestServer::start();
    let mut players = Vec::new();
    for i in 0..9 {
        players.push(guest(server.addr, &format!("p{i}"), 80, 24).await);
    }
    for p in &mut players {
        quick_play(p).await;
    }
    let mut matched = 0;
    for p in &mut players {
        if p.saw("GET READY", Duration::from_secs(4)).await {
            matched += 1;
        } else {
            assert!(p.text().contains("Finding a match"), "{}", p.text());
        }
    }
    assert_eq!(matched, 8);
}

#[tokio::test]
async fn wrong_codes_are_refused_and_the_sixth_try_is_blocked() {
    let server = TestServer::start();
    let mut p = guest(server.addr, "ann", 80, 24).await;
    pick(&p, 3).await;
    p.wait_for("Join with a code").await;
    for i in 2..=6 {
        let code = format!("ZZZZZ{i}");
        // Typing clears the last error, so each one seen is new.
        type_code(&p, &code).await;
        p.wait_until("the code typed", Duration::from_secs(5), |p| {
            let text = p.text();
            text.contains(&format!("{code}▏")) && !text.contains("No room has")
        })
        .await;
        p.send(ENTER).await;
        p.wait_for("No room has that code.").await;
    }
    p.send(ENTER).await;
    p.wait_for("Too many wrong codes").await;
}

#[tokio::test]
async fn small_terminals_are_warned_and_see_part_of_the_board() {
    let server = TestServer::start();
    let mut ann = guest(server.addr, "ann", 100, 30).await;
    let mut ben = guest(server.addr, "ben", 50, 16).await;
    pick(&ben, 1).await;
    ben.wait_for("Your terminal is 50×16").await;
    // Esc clears the warning; the menu is still on Quick play.
    ben.send(ESC).await;
    ben.wait_until("the warning cleared", Duration::from_secs(2), |p| {
        p.text().contains("Playing as")
    })
    .await;

    let code = create_room(&mut ann).await;
    pick(&ben, 2).await;
    ben.wait_for("Join with a code").await;
    type_code(&ben, &code).await;
    ben.send(ENTER).await;
    ben.wait_for("Private room").await;
    for p in [&ann, &ben] {
        p.send(ENTER).await;
    }
    ben.wait_for("showing part of the board").await;
    ann.wait_for("GET READY").await;
    assert!(!ann.text().contains("showing part"));
}

#[tokio::test]
async fn a_new_nickname_shows_and_stays_with_the_key() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = Player::join(server.addr, Login::Key(&key), 80, 24).await;
    p.wait_for("~alice").await;
    pick(&p, 4).await;
    p.wait_for("alice▏").await;
    for _ in 0..5 {
        p.send(BACKSPACE).await;
    }
    // Escape sequences and other characters that could draw on other
    // players' screens never get into a name.
    p.send(b"ze\x1b[31md!").await;
    p.wait_for("zed▏").await;
    p.send(ENTER).await;
    p.wait_for("Playing as ~zed").await;
    drop(p);
    sleep(Duration::from_millis(200)).await;

    let mut again = Player::join(server.addr, Login::Key(&key), 80, 24).await;
    again.wait_for("Playing as ~zed").await;
    // Names that say nothing get a generated one.
    pick(&again, 4).await;
    again.wait_for("zed▏").await;
    for _ in 0..3 {
        again.send(BACKSPACE).await;
    }
    again.send(b"root").await;
    again.send(ENTER).await;
    again.wait_for("Playing as ~player-").await;
}

/// Fake players that queue, steer at random and queue again, for the load
/// check. Run with:
/// `cargo test --release -p snaketui-server --test multiplayer -- --ignored`
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a load check; run it on its own, in a release build"]
async fn two_hundred_players_keep_rooms_on_time() {
    const BOTS: usize = 200;
    let server = TestServer::with(|c| {
        c.workers = 0;
        c.max_sessions_per_ip = BOTS + 10;
    });
    let stop = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(AtomicU64::new(0));
    let mut bots = Vec::new();
    for i in 0..BOTS {
        let p = guest(server.addr, &format!("bot{i}"), 80, 24).await;
        let (stop, bytes) = (Arc::clone(&stop), Arc::clone(&bytes));
        bots.push(tokio::spawn(async move {
            let (mut rx, tx) = p.channel.split();
            let reader = tokio::spawn(async move {
                while let Some(msg) = rx.wait().await {
                    if let ChannelMsg::Data { data } = msg {
                        bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
                    }
                }
            });
            let _ = tx.data_bytes(DOWN.to_vec()).await;
            let _ = tx.data_bytes(ENTER.to_vec()).await;
            let keys = [UP, LEFT, DOWN, RIGHT];
            let mut n = i;
            while !stop.load(Ordering::Relaxed) {
                sleep(Duration::from_millis(150)).await;
                n = n.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                // Mostly steering; now and then Enter, which queues again
                // after a match.
                let key = if n % 13 == 0 { ENTER } else { keys[n / 7 % 4] };
                if tx.data_bytes(key.to_vec()).await.is_err() {
                    break;
                }
            }
            reader.abort();
            drop(p._client);
        }));
    }
    // Let matches form, then measure.
    sleep(Duration::from_secs(5)).await;
    TICK_LAG.reset();
    let start = Instant::now();
    let before = bytes.load(Ordering::Relaxed);
    sleep(Duration::from_secs(30)).await;
    let sent = bytes.load(Ordering::Relaxed) - before;
    let (p50, p99, ticks) = (
        TICK_LAG.quantile(0.5).unwrap(),
        TICK_LAG.quantile(0.99).unwrap(),
        TICK_LAG.count(),
    );
    stop.store(true, Ordering::Relaxed);
    for bot in bots {
        let _ = bot.await;
    }
    eprintln!(
        "{BOTS} players: {ticks} room ticks in {:?}, lag p50 {p50:?}, p99 {p99:?}; \
         {} KiB/s to players",
        start.elapsed(),
        sent / 1024 / 30
    );
    assert!(p99 < Duration::from_millis(5), "p99 tick lag {p99:?}");
}

/// The lobby line naming `who`.
fn lobby_line(p: &Player, who: &str) -> String {
    p.text()
        .lines()
        .find(|l| l.contains(who))
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn a_host_plays_bots_in_a_private_room() {
    let server = TestServer::start();
    let mut ann = guest(server.addr, "ann", 100, 30).await;
    create_room(&mut ann).await;
    ann.send(b"b").await;
    ann.wait_for("Viper (bot)").await;
    ann.send(b"b").await;
    ann.wait_for("Cobra (bot)").await;
    ann.send(b"x").await;
    ann.wait_until("Cobra out", Duration::from_secs(5), |p| {
        !p.text().contains("Cobra")
    })
    .await;
    // Bots are always ready, so ann's Enter starts the round.
    ann.send(ENTER).await;
    ann.wait_for("GET READY").await;
    // She never steers: she's soon out, and the round with her.
    ann.wait_until("the results", Duration::from_secs(20), |p| {
        p.text().contains("RESULTS")
    })
    .await;
    assert!(ann.text().contains("Viper (bot)"), "{}", ann.text());
}

#[tokio::test]
async fn the_host_sets_the_rules_for_everyone() {
    let server = TestServer::start();
    let mut ann = guest(server.addr, "ann", 100, 30).await;
    let mut ben = guest(server.addr, "ben", 100, 30).await;
    let code = create_room(&mut ann).await;
    join_room(&mut ben, &code).await;
    ann.wait_for("◂ 150 ms ticks ▸").await;

    // ben isn't the host: his arrows change nothing. He gets ready.
    ben.send(RIGHT).await;
    ben.send(ENTER).await;
    ben.wait_until("ben ready", Duration::from_secs(5), |p| {
        lobby_line(p, "~ben").contains('✓')
    })
    .await;
    assert!(ben.text().contains("150 ms ticks"), "{}", ben.text());

    // ann makes it faster, a step at a time.
    ann.send(LEFT).await;
    ann.wait_for("◂ 125 ms ticks ▸").await;
    ann.send(LEFT).await;
    ann.wait_for("◂ 100 ms ticks ▸").await;
    ben.wait_for("100 ms ticks").await;
    assert!(
        !lobby_line(&ben, "~ben").contains('✓'),
        "ben was ready for another speed:\n{}",
        ben.text()
    );

    // Any rule: board width, height, tick speed, then edge walls.
    ann.send(b"e").await;
    ann.wait_for("Room rules").await;
    for _ in 0..3 {
        ann.send(DOWN).await;
    }
    ann.send(RIGHT).await;
    ann.wait_for("◂ none (wrap) ▸").await;
    ben.wait_for("~ann set edge walls to none (wrap).").await;
    ben.send(b"e").await;
    ben.wait_for("Only ~ann can change these.").await;
    assert!(ben.text().contains("none (wrap)"), "{}", ben.text());
}
