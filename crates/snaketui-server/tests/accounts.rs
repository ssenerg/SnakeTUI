//! Accounts end to end: signing up and in, remembered keys, friends,
//! invites and ratings.

mod common;

use std::time::Duration;

use common::accounts::*;
use common::*;
use snake_core::round::{Entrant, Room, RoomKind, RoomSettings, quick_rules};
use snaketui_server::rating::Rating;
use snaketui_server::store::{RatedResult, Store};

#[tokio::test]
async fn a_new_account_signs_in_by_itself_from_the_same_key() {
    let server = TestServer::start();
    let (p, key) = signed_up(server.addr, "ann").await;
    drop(p);
    let again = connect_as(server.addr, Login::Key(&key)).await;
    assert!(
        again.text().contains("Signed in as ann"),
        "{}",
        again.text()
    );

    // Another key is a guest until it signs in, and then it's remembered.
    let other = new_key();
    let mut p = connect_as(server.addr, Login::Key(&other)).await;
    assert!(p.text().contains("(guest)"), "{}", p.text());
    assert!(!p.text().contains("Friends"), "guests have no friends list");
    try_sign_in(&mut p, "ANN", PASSWORD, true).await;
    p.wait_for("Signed in as ann").await;
    assert!(p.text().contains("Friends"), "{}", p.text());
    drop(p);
    let remembered = connect_as(server.addr, Login::Key(&other)).await;
    assert!(remembered.text().contains("Signed in as ann"));
}

#[tokio::test]
async fn usernames_are_unique_and_some_are_reserved() {
    let server = TestServer::start();
    let _ann = signed_up(server.addr, "ann").await;
    let mut p = connect_as(server.addr, Login::Guest).await;
    pick(&p, 0, SIGN_UP).await;
    for (name, problem) in [("Ann", "That username is taken."), ("root", "reserved")] {
        p.send(name.as_bytes()).await;
        p.send(ENTER).await;
        p.send(PASSWORD.as_bytes()).await;
        p.send(ENTER).await;
        p.send(PASSWORD.as_bytes()).await;
        p.send(ENTER).await;
        p.wait_for(problem).await;
        for _ in 0..name.len() {
            p.send(BACKSPACE).await;
        }
    }
}

#[tokio::test]
async fn wrong_passwords_are_refused_and_the_fifth_disconnects() {
    let server = TestServer::start();
    let _ann = signed_up(server.addr, "ann").await;
    let mut p = connect_as(server.addr, Login::Guest).await;
    try_sign_in(&mut p, "ann", "not the password", false).await;
    p.wait_for(WRONG).await;
    // A name nobody has gets the same answer.
    retry_sign_in(&mut p, "nobody", PASSWORD).await;
    p.wait_for(WRONG).await;
    for _ in 0..2 {
        retry_sign_in(&mut p, "ann", "wrong again").await;
        p.wait_for(WRONG).await;
    }
    retry_sign_in(&mut p, "ann", "and again").await;
    assert_eq!(p.wait_closed(Duration::from_secs(5)).await, Some(1));
    assert!(p.text().contains("Too many wrong sign-ins"), "{}", p.text());
}

#[tokio::test]
async fn signing_out_forgets_the_key() {
    let server = TestServer::start();
    let (mut p, key) = signed_up(server.addr, "ann").await;
    pick(&p, QUICK_PLAY, ACCOUNT).await;
    p.wait_for("SHA256:").await;
    assert!(p.text().contains("  this machine"), "{}", p.text());
    // The key, then Change password, Sign out, Back.
    down_enter(&p, 2).await;
    p.wait_for("Signed out.").await;
    assert!(p.text().contains("Sign up"), "{}", p.text());
    drop(p);
    let again = connect_as(server.addr, Login::Key(&key)).await;
    assert!(again.text().contains("(guest)"), "{}", again.text());
}

#[tokio::test]
async fn a_guests_settings_and_records_move_into_a_new_account() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = connect_as(server.addr, Login::Key(&key)).await;
    p.single_player().await;
    p.send(b"d").await; // board width 30 -> 31
    p.wait_for("◂ 31 ▸").await;
    p.send(ENTER).await;
    p.wait_for("press an arrow key").await;
    p.send(ESC).await;
    p.wait_for("PAUSED").await;
    p.send(b"q").await; // the game counts toward the record on the way out
    p.wait_for("best for this setup: 3").await;
    p.send(b"q").await;
    p.wait_for("Quick play").await;
    sign_up(&mut p, "ann").await;
    drop(p);

    // Signed in without a key, it's all there.
    let mut elsewhere = connect_as(server.addr, Login::Guest).await;
    try_sign_in(&mut elsewhere, "ann", PASSWORD, false).await;
    elsewhere.wait_for("Signed in as ann").await;
    elsewhere.send(UP).await; // from Quick play to Single player
    elsewhere.send(ENTER).await;
    elsewhere.wait_for("◂ 31 ▸").await;
    elsewhere.wait_for("best for this setup: 3").await;
}

#[tokio::test]
async fn friends_ask_accept_and_invite_each_other_into_a_room() {
    let server = TestServer::start();
    let (mut ann, _) = signed_up(server.addr, "ann").await;
    let (mut ben, _) = signed_up(server.addr, "ben").await;
    make_friends(&mut ann, &mut ben).await;
    // ann's list now shows ben, online.
    ann.wait_until("ben online", Duration::from_secs(5), |p| {
        line_of(&p.text(), "ben").contains("online")
    })
    .await;

    ann.send(DOWN).await;
    ann.send(ENTER).await;
    ann.wait_for("Private room").await;
    ann.wait_for("Invited ben.").await;
    ben.wait_for("ann invites you to play").await;
    ben.send(ENTER).await;
    ben.wait_for("Private room").await;
    for p in [&mut ann, &mut ben] {
        p.wait_until("both in the room", Duration::from_secs(5), |p| {
            let text = p.text();
            text.contains("ann") && text.contains("ben") && !text.contains("invites you")
        })
        .await;
    }
}

#[tokio::test]
async fn only_friends_can_be_invited() {
    let server = TestServer::start();
    let (mut ann, _) = signed_up(server.addr, "ann").await;
    let (mut ben, _) = signed_up(server.addr, "ben").await;
    make_friends(&mut ann, &mut ben).await;
    ann.wait_until("ben online", Duration::from_secs(5), |p| {
        line_of(&p.text(), "ben").contains("online")
    })
    .await;

    // ben, still on ann's row, ends the friendship once his list shows her
    // as a friend; ann's list hasn't caught up yet.
    ben.wait_until("ann as a friend", Duration::from_secs(5), |p| {
        line_of(&p.text(), "ann").contains("online")
    })
    .await;
    ben.send(b"x").await;
    ben.wait_for("Press x again to remove ann").await;
    ben.send(b"x").await;
    ben.wait_until("ann gone", Duration::from_secs(5), |p| {
        !p.text().contains("ann")
    })
    .await;
    ann.send(DOWN).await;
    ann.send(ENTER).await;
    ann.wait_for("You can only invite friends.").await;
    ben.send(ESC).await;
    assert!(!ben.saw("invites you", Duration::from_millis(300)).await);
}

/// The first seed whose quick-play match, with nobody pressing a key, is
/// won by the player who queued first.
fn first_player_wins_seed() -> u64 {
    let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
    let tick = Duration::from_millis(settings.game.tick_ms);
    (0..10_000)
        .find(|&seed| {
            let players = [1, 2]
                .map(|id| Entrant {
                    id,
                    name: id.to_string(),
                })
                .to_vec();
            let mut round = Room::new_match(settings.clone(), seed, players);
            for _ in 0..2_000 {
                if let Some(end) = round.advance(tick) {
                    return game_api::winner(end.placements()).is_some_and(|w| w.id == 1);
                }
            }
            false
        })
        .expect("some seed works")
}

#[tokio::test]
async fn the_leaderboard_lists_players_after_five_rated_games() {
    let server = TestServer::start();
    let mut keys = Vec::new();
    for name in ["ann", "ben", "cat"] {
        let (_, key) = signed_up(server.addr, name).await;
        keys.push(key);
    }
    // What their rated rounds would have left, written behind the
    // server's back; they sign in again below to see it.
    let store = Store::open(&format!(
        "sqlite://{}",
        server.dir.join("db.sqlite").display()
    ))
    .await
    .unwrap();
    for (name, mu, games) in [("ann", 26.0, 5), ("ben", 27.5, 6), ("cat", 30.0, 3)] {
        let (account, _) = store.find_account(name).await.unwrap().unwrap();
        for _ in 0..games {
            let results = [RatedResult {
                account: account.id,
                rating: Rating { mu, sigma: 4.0 },
                won: true,
            }];
            store.save_rated_round(&results).await.unwrap();
        }
    }
    store.close().await;

    let mut guest = connect_as(server.addr, Login::Guest).await;
    pick(&guest, 0, GUEST_LEADERBOARD).await;
    guest.wait_for("Sign up to get a rating.").await;
    let text = guest.text();
    let row = |place: &str, name: &str, rating: &str| {
        text.lines()
            .any(|l| l.contains(&format!("{place}  {name} ")) && l.contains(rating))
    };
    assert!(row("1", "ben", "1100") && row("2", "ann", "1040"), "{text}");
    assert!(!text.contains("cat"), "3 games aren't enough: {text}");

    let mut ben = connect_as(server.addr, Login::Key(&keys[1])).await;
    pick(&ben, 0, LEADERBOARD).await;
    ben.wait_for("You: 1st · 1100 · 6 games").await;
    let mut cat = connect_as(server.addr, Login::Key(&keys[2])).await;
    pick(&cat, 0, LEADERBOARD).await;
    cat.wait_for("2 more rated games to be ranked.").await;
    cat.send(ESC).await;
    cat.wait_for("Signed in as cat").await;
}

/// The rating in the home menu's "Signed in as … · rating N".
fn rating_on_home(text: &str) -> Option<u32> {
    let at = text.find("· rating ")? + "· rating ".len();
    let digits: String = text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[tokio::test]
async fn a_flood_of_keys_leaves_the_server_responsive() {
    let server = TestServer::start();
    let mut flood = connect_as(server.addr, Login::Guest).await;
    pick(&flood, 0, GUEST_LEADERBOARD).await;
    flood.wait_for("Sign up to get a rating.").await;
    // Every Enter would open the leaderboard again, or go back home: a
    // query each, without the server's limits.
    let enters = vec![b'\r'; 16 * 1024];
    let started = std::time::Instant::now();
    for _ in 0..10 {
        flood.send(&enters).await;
    }
    let mut other = connect_as(server.addr, Login::Guest).await;
    pick(&other, 0, GUEST_LEADERBOARD).await;
    other.wait_for("Sign up to get a rating.").await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    // The flooder is still served too: Enter takes them home from the
    // leaderboard, or back to it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = flood.text();
    flood.send(ENTER).await;
    flood
        .wait_until(
            "the flooder's screen to change",
            Duration::from_secs(3),
            |p| p.text() != before,
        )
        .await;
}

#[tokio::test]
async fn friends_watch_each_others_games() {
    let server = TestServer::start();
    let (mut ann, _) = signed_up(server.addr, "ann").await;
    let (mut ben, _) = signed_up(server.addr, "ben").await;
    make_friends(&mut ann, &mut ben).await;

    // ben makes a room, with a bot to play.
    ben.send(ESC).await;
    ben.wait_for("Signed in as ben").await;
    ben.send(UP).await;
    ben.send(UP).await;
    ben.send(ENTER).await;
    ben.wait_for("Private room").await;
    ben.send(b"b").await;
    ben.wait_for("Viper (bot)").await;

    // ann opens her friends again and sees him there.
    ann.send(ESC).await;
    ann.wait_for("Signed in as ann").await;
    ann.send(ENTER).await;
    ann.wait_for("in a private room · v watch").await;
    ann.send(DOWN).await;
    ann.send(b"v").await;
    ann.wait_for("Watching ben").await;
    ben.wait_for("1 watching").await;
    assert!(!ann.text().contains("Enter ready"), "{}", ann.text());

    // The round starts for ben; ann follows his snake.
    ben.send(ENTER).await;
    ann.wait_for("ben is").await;
    ann.send(ESC).await;
    ann.wait_for("Add a friend").await;
    ben.wait_until("nobody watching", Duration::from_secs(5), |p| {
        !p.text().contains("watching")
    })
    .await;
}

#[tokio::test]
async fn quick_play_offers_an_unrated_match_against_bots() {
    let server = TestServer::start();
    let (mut ann, _) = signed_up(server.addr, "ann").await;
    ann.send(ENTER).await;
    ann.wait_for("Finding a match").await;
    assert!(!ann.text().contains("bots"), "{}", ann.text());
    ann.wait_until("bots offered", Duration::from_secs(30), |p| {
        p.text().contains("b play bots")
    })
    .await;
    ann.send(b"b").await;
    ann.wait_until("the results", Duration::from_secs(30), |p| {
        p.text().contains("RESULTS")
    })
    .await;
    let text = ann.text();
    assert!(text.contains("(bot)"), "{text}");
    assert!(!line_of(&text, ". ann ").contains('−'), "unrated: {text}");
    ann.send(ESC).await;
    ann.wait_for("Signed in as ann").await;
    assert_eq!(rating_on_home(&ann.text()), Some(1000), "{}", ann.text());
}

#[tokio::test]
async fn beating_guests_doesnt_move_a_rating() {
    let seed = first_player_wins_seed();
    let server = TestServer::with(|c| c.room_seed = Some(seed));
    let (mut ann, _) = signed_up(server.addr, "ann").await;
    ann.send(ENTER).await;
    ann.wait_for("Finding a match").await;
    let mut guest = connect_as(server.addr, Login::Guest).await;
    pick(&guest, 0, QUICK_PLAY).await;
    for p in [&mut ann, &mut guest] {
        p.wait_until("the results", Duration::from_secs(30), |p| {
            p.text().contains("RESULTS")
        })
        .await;
    }
    assert!(ann.text().contains("You win!"), "{}", ann.text());
    assert!(
        !line_of(&ann.text(), ". ann ").contains('+'),
        "{}",
        ann.text()
    );
    ann.send(ESC).await;
    ann.wait_for("Signed in as ann").await;
    assert_eq!(rating_on_home(&ann.text()), Some(1000), "{}", ann.text());
}

#[tokio::test]
async fn a_rated_match_moves_the_winners_rating_up_and_the_losers_down() {
    let seed = first_player_wins_seed();
    let server = TestServer::with(|c| c.room_seed = Some(seed));
    let (mut ann, key) = signed_up(server.addr, "ann").await;
    let (mut ben, _) = signed_up(server.addr, "ben").await;
    ann.send(ENTER).await;
    ann.wait_for("Finding a match").await;
    ben.send(ENTER).await;
    for p in [&mut ann, &mut ben] {
        p.wait_until("the results", Duration::from_secs(30), |p| {
            p.text().contains("RESULTS")
        })
        .await;
    }
    assert!(ann.text().contains("You win!"), "{}", ann.text());
    for p in [&mut ann, &mut ben] {
        p.wait_until("the rating changes", Duration::from_secs(3), |p| {
            let text = p.text();
            line_of(&text, ". ann ").contains('+') && line_of(&text, ". ben ").contains('−')
        })
        .await;
        p.send(ESC).await;
    }
    ann.wait_until("a higher rating", Duration::from_secs(3), |p| {
        rating_on_home(&p.text()).is_some_and(|r| r > 1000)
    })
    .await;
    ben.wait_until("a lower rating", Duration::from_secs(3), |p| {
        rating_on_home(&p.text()).is_some_and(|r| r < 1000)
    })
    .await;

    // It's saved.
    let rating = rating_on_home(&ann.text());
    drop(ann);
    let again = connect_as(server.addr, Login::Key(&key)).await;
    assert_eq!(rating_on_home(&again.text()), rating, "{}", again.text());
}
