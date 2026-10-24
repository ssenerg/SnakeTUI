//! Several servers working as one: accounts, keys and guessing limits
//! through a shared Postgres database, and rooms, presence and invites
//! through NATS. The NATS tests run real server processes, so a server can
//! be killed outright.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use common::accounts::*;
use common::process::{MatchmakerProcess, ServerProcess};
use common::rooms::*;
use common::*;
use tokio::time::Instant;

async fn two_servers() -> (TestServer, TestServer) {
    let db = snaketui_test_services::postgres().await;
    let a = TestServer::with(|c| c.database = db.clone());
    let b = TestServer::with(|c| c.database = db.clone());
    (a, b)
}

#[tokio::test]
async fn an_account_made_on_one_server_signs_in_on_another() {
    let (a, b) = two_servers().await;
    let (p, key) = signed_up(a.addr, "ann").await;
    drop(p);
    let on_b = connect_as(b.addr, Login::Key(&key)).await;
    assert!(on_b.text().contains("Signed in as ann"), "{}", on_b.text());
    drop(on_b);

    // A new machine signs in on B with the password; A knows it after.
    let other = new_key();
    let mut p = connect_as(b.addr, Login::Key(&other)).await;
    try_sign_in(&mut p, "ann", PASSWORD, true).await;
    p.wait_for("Signed in as ann").await;
    drop(p);
    let back_on_a = connect_as(a.addr, Login::Key(&other)).await;
    assert!(
        back_on_a.text().contains("Signed in as ann"),
        "{}",
        back_on_a.text()
    );
}

#[tokio::test]
async fn a_username_locked_on_one_server_is_locked_on_all() {
    let (a, b) = two_servers().await;
    let _ann = signed_up(a.addr, "ann").await;
    let mut p = connect_as(a.addr, Login::Guest).await;
    try_sign_in(&mut p, "ann", "wrong guess 1", false).await;
    p.wait_for(WRONG).await;
    for i in 2..=4 {
        retry_sign_in(&mut p, "ann", &format!("wrong guess {i}")).await;
        p.wait_for(WRONG).await;
    }
    retry_sign_in(&mut p, "ann", "wrong guess 5").await;
    p.wait_closed(Duration::from_secs(5)).await;

    // Even the right password, on the other server, has to wait.
    let mut q = connect_as(b.addr, Login::Guest).await;
    try_sign_in(&mut q, "ann", PASSWORD, false).await;
    q.wait_for("Too many wrong tries for that name").await;
}

/// Servers in one cluster, one in each of `regions`, with a database and
/// NATS names of their own. Also gives settings a matchmaker for the
/// cluster can use.
async fn cluster_in(regions: &[&str]) -> (Vec<ServerProcess>, String) {
    static N: AtomicU32 = AtomicU32::new(0);
    let db = snaketui_test_services::postgres().await;
    let nats = snaketui_test_services::nats();
    let name = format!(
        "t{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let settings = |region: &str| {
        format!(
            "database = \"{db}\"\n\n[cluster]\nnats = \"{nats}\"\nname = \"{name}\"\nregion = \"{region}\"\n"
        )
    };
    let mut servers: Vec<ServerProcess> = regions
        .iter()
        .map(|region| ServerProcess::spawn(&settings(region)))
        .collect();
    for server in &mut servers {
        server.ready();
    }
    // Long enough for each to have heard from all the others.
    tokio::time::sleep(Duration::from_secs(1)).await;
    (servers, settings("local"))
}

/// `n` servers in one cluster and one region.
async fn cluster(n: usize) -> Vec<ServerProcess> {
    cluster_in(&vec!["local"; n]).await.0
}

/// Queues a guest from a fresh home menu.
async fn quick_play(p: &mut Player) {
    p.send(DOWN).await;
    p.send(ENTER).await;
    p.wait_for("Finding a match").await;
}

/// Waits until `p`'s match has started its countdown.
async fn in_a_match(p: &mut Player, limit: Duration) {
    p.wait_until("a match", limit, |p| p.text().contains("GET READY"))
        .await;
}

async fn results(p: &mut Player) {
    p.wait_until("the results", Duration::from_secs(40), |p| {
        p.text().contains("RESULTS")
    })
    .await;
}

/// Waits until `p` sees every one of `names`.
async fn sees_all(p: &mut Player, names: &[&str]) {
    p.wait_until("everyone in the room", Duration::from_secs(5), |p| {
        let text = p.text();
        names.iter().all(|n| text.contains(n))
    })
    .await;
}

#[tokio::test]
async fn players_on_two_servers_share_a_private_room() {
    let servers = cluster(2).await;
    let mut ann = guest(servers[0].addr, "ann", 100, 30).await;
    let mut ben = guest(servers[1].addr, "ben", 100, 30).await;
    let code = create_room(&mut ann).await;
    join_room(&mut ben, &code).await;
    for p in [&mut ann, &mut ben] {
        sees_all(p, &["~ann", "~ben"]).await;
    }
    // ben's key reaches the room on the other server: with both ready, the
    // countdown starts now rather than after 15 seconds.
    ann.send(ENTER).await;
    ben.send(ENTER).await;
    for p in [&mut ann, &mut ben] {
        p.wait_until("the countdown", Duration::from_secs(5), |p| {
            p.text().contains("GET READY")
        })
        .await;
    }
}

#[tokio::test]
async fn when_a_players_server_dies_the_room_plays_on() {
    let mut servers = cluster(2).await;
    let mut ann = guest(servers[0].addr, "ann", 100, 30).await;
    let mut cat = guest(servers[0].addr, "cat", 100, 30).await;
    let mut ben = guest(servers[1].addr, "ben", 100, 30).await;
    let code = create_room(&mut ann).await;
    join_room(&mut cat, &code).await;
    join_room(&mut ben, &code).await;
    sees_all(&mut ann, &["~ann", "~ben", "~cat"]).await;

    servers[1].kill();
    ann.wait_until("ben gone", Duration::from_secs(4), |p| {
        let text = p.text();
        !text.contains("~ben") && text.contains("~cat")
    })
    .await;
    assert!(ann.text().contains("Private room"), "{}", ann.text());
}

#[tokio::test]
async fn when_a_rooms_server_dies_its_players_elsewhere_hear_within_3_seconds() {
    let mut servers = cluster(2).await;
    let mut ann = guest(servers[0].addr, "ann", 100, 30).await;
    let mut ben = guest(servers[1].addr, "ben", 100, 30).await;
    let code = create_room(&mut ann).await;
    join_room(&mut ben, &code).await;
    sees_all(&mut ben, &["~ann", "~ben"]).await;

    servers[0].kill();
    ben.wait_until("word that the room is gone", Duration::from_secs(3), |p| {
        p.text().contains("went away")
    })
    .await;
    assert!(!ben.text().contains("Private room"), "{}", ben.text());
}

#[tokio::test]
async fn friends_on_different_servers_see_each_other_and_play_together() {
    let servers = cluster(2).await;
    let (mut ann, _) = signed_up(servers[0].addr, "ann").await;
    let (mut ben, _) = signed_up(servers[1].addr, "ben").await;
    // The request and the answer each reach the other server at once.
    make_friends(&mut ann, &mut ben).await;
    ann.wait_until("ben online", Duration::from_secs(5), |p| {
        line_of(&p.text(), "ben").contains("online")
    })
    .await;

    ann.send(DOWN).await;
    ann.send(ENTER).await;
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
async fn quick_play_matches_within_a_region_first_then_across_regions() {
    let (servers, settings) = cluster_in(&["eu", "eu", "us"]).await;
    // Five times faster: matches fill for 2 s, and regions open after 4 s.
    let _queue = MatchmakerProcess::start(&settings, 5.0);
    let mut ann = guest(servers[0].addr, "ann", 100, 30).await;
    let mut ben = guest(servers[2].addr, "ben", 100, 30).await;
    let mut cat = guest(servers[1].addr, "cat", 100, 30).await;
    for p in [&mut ann, &mut ben, &mut cat] {
        quick_play(p).await;
    }
    for p in [&mut ann, &mut cat] {
        in_a_match(p, Duration::from_secs(10)).await;
        let text = p.text();
        assert!(text.contains("~ann") && text.contains("~cat"), "{text}");
    }
    assert!(ben.text().contains("Finding a match"), "{}", ben.text());
    assert!(
        !servers[2].log().contains("hosting a match"),
        "a match of eu players runs in eu"
    );

    // ben, alone in us, plays dan from eu once both have waited for it.
    let mut dan = guest(servers[0].addr, "dan", 100, 30).await;
    quick_play(&mut dan).await;
    for p in [&mut ben, &mut dan] {
        in_a_match(p, Duration::from_secs(15)).await;
        let text = p.text();
        assert!(text.contains("~ben") && text.contains("~dan"), "{text}");
    }
    // The round plays out for both, on two servers.
    for p in [&mut ben, &mut dan] {
        results(p).await;
    }
}

#[tokio::test]
async fn a_restarted_matchmaker_loses_no_waiting_players() {
    let (servers, settings) = cluster_in(&["eu", "eu"]).await;
    let mut queue = MatchmakerProcess::start(&settings, 1.0);
    let mut ann = guest(servers[0].addr, "ann", 100, 30).await;
    let mut ben = guest(servers[1].addr, "ben", 100, 30).await;
    quick_play(&mut ann).await;
    quick_play(&mut ben).await;
    let queued = Instant::now();
    tokio::time::sleep(Duration::from_secs(1)).await;
    queue.kill();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _queue = MatchmakerProcess::start(&settings, 1.0);

    // They kept their place: matched 10 s after they queued, not 10 s
    // after the restart.
    for p in [&mut ann, &mut ben] {
        in_a_match(p, Duration::from_secs(15)).await;
    }
    let after = queued.elapsed();
    assert!(
        after < Duration::from_millis(12_500),
        "matched after {after:?}"
    );
}

#[tokio::test]
async fn a_match_across_servers_counts_for_both_players() {
    let (servers, settings) = cluster_in(&["eu", "eu"]).await;
    let _queue = MatchmakerProcess::start(&settings, 5.0);
    let (mut ann, _) = signed_up(servers[0].addr, "ann").await;
    let (mut ben, _) = signed_up(servers[1].addr, "ben").await;
    // Signing up leaves the cursor on Quick play.
    for p in [&mut ann, &mut ben] {
        p.send(ENTER).await;
    }
    for p in [&mut ann, &mut ben] {
        in_a_match(p, Duration::from_secs(10)).await;
    }
    for p in [&mut ann, &mut ben] {
        results(p).await;
    }
    // Whichever server ran it, both players' games were counted, and the
    // one elsewhere heard about it.
    for p in [&mut ann, &mut ben] {
        p.send(ESC).await;
        p.wait_for("Signed in as").await;
        pick(p, QUICK_PLAY, ACCOUNT).await;
        p.wait_for("1 games").await;
    }
}
