//! Accounts end to end: signing up and in, remembered keys, friends,
//! invites and ratings.

mod common;

use std::time::Duration;

use common::accounts::*;
use common::*;
use snake_core::round::{
    Entrant, Outcome, RoomKind, Round, RoundEvent, RoundSettings, quick_rules,
};

#[tokio::test]
async fn a_new_account_signs_in_by_itself_from_the_same_key() {
    let server = TestServer::start();
    let (p, key) = signed_up(&server, "ann").await;
    drop(p);
    let again = connect_as(&server, Login::Key(&key)).await;
    assert!(
        again.text().contains("Signed in as ann"),
        "{}",
        again.text()
    );

    // Another key is a guest until it signs in, and then it's remembered.
    let other = new_key();
    let mut p = connect_as(&server, Login::Key(&other)).await;
    assert!(p.text().contains("(guest)"), "{}", p.text());
    assert!(!p.text().contains("Friends"), "guests have no friends list");
    try_sign_in(&mut p, "ANN", PASSWORD, true).await;
    p.wait_for("Signed in as ann").await;
    assert!(p.text().contains("Friends"), "{}", p.text());
    drop(p);
    let remembered = connect_as(&server, Login::Key(&other)).await;
    assert!(remembered.text().contains("Signed in as ann"));
}

#[tokio::test]
async fn usernames_are_unique_and_some_are_reserved() {
    let server = TestServer::start();
    let _ann = signed_up(&server, "ann").await;
    let mut p = connect_as(&server, Login::Guest).await;
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
    let _ann = signed_up(&server, "ann").await;
    let mut p = connect_as(&server, Login::Guest).await;
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
    let (mut p, key) = signed_up(&server, "ann").await;
    pick(&p, QUICK_PLAY, ACCOUNT).await;
    p.wait_for("SHA256:").await;
    assert!(p.text().contains("  this machine"), "{}", p.text());
    // The key, then Change password, Sign out, Back.
    down_enter(&p, 2).await;
    p.wait_for("Signed out.").await;
    assert!(p.text().contains("Sign up"), "{}", p.text());
    drop(p);
    let again = connect_as(&server, Login::Key(&key)).await;
    assert!(again.text().contains("(guest)"), "{}", again.text());
}

#[tokio::test]
async fn a_guests_settings_and_records_move_into_a_new_account() {
    let server = TestServer::start();
    let key = new_key();
    let mut p = connect_as(&server, Login::Key(&key)).await;
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
    let mut elsewhere = connect_as(&server, Login::Guest).await;
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
    let (mut ann, _) = signed_up(&server, "ann").await;
    let (mut ben, _) = signed_up(&server, "ben").await;
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
    let (mut ann, _) = signed_up(&server, "ann").await;
    let (mut ben, _) = signed_up(&server, "ben").await;
    make_friends(&mut ann, &mut ben).await;
    ann.wait_until("ben online", Duration::from_secs(5), |p| {
        line_of(&p.text(), "ben").contains("online")
    })
    .await;

    // ben, still on ann's row, ends the friendship; ann's list hasn't
    // caught up yet.
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
    let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
    let tick = Duration::from_millis(settings.rules.tick_ms);
    (0..10_000)
        .find(|&seed| {
            let players = [1, 2]
                .map(|id| Entrant {
                    id,
                    name: id.to_string(),
                })
                .to_vec();
            let mut round = Round::new_match(settings.clone(), seed, players);
            for _ in 0..2_000 {
                if let [RoundEvent::Ended { outcome, .. }] = round.advance(tick).as_slice() {
                    return matches!(outcome, Outcome::Won { winner, .. } if winner.id == 1);
                }
            }
            false
        })
        .expect("some seed works")
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
async fn a_rated_match_moves_the_winners_rating_up_and_the_losers_down() {
    let seed = first_player_wins_seed();
    let server = TestServer::with(|c| c.room_seed = Some(seed));
    let (mut ann, key) = signed_up(&server, "ann").await;
    let (mut ben, _) = signed_up(&server, "ben").await;
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
    let again = connect_as(&server, Login::Key(&key)).await;
    assert_eq!(rating_on_home(&again.text()), rating, "{}", again.text());
}
