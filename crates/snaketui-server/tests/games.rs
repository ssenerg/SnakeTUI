//! Which game opens: the one `ssh <game>@host` names, or else the one the
//! player had open last time, kept for their key or account.

mod common;

use std::time::Duration;

use common::accounts::{connect_as, sign_up};
use common::*;
use game_api::GameId;
use russh::keys::{HashAlg, PrivateKey};
use snaketui_server::store::Store;
use tokio::time::{Instant, sleep};

fn fingerprint(key: &PrivateKey) -> String {
    key.public_key().fingerprint(HashAlg::Sha256).to_string()
}

async fn store(server: &TestServer) -> Store {
    let url = format!("sqlite://{}", server.dir.join("db.sqlite").display());
    Store::open(&url).await.unwrap()
}

/// What `read` finds saved, once it finds something: the server saves the
/// game a moment after it opens. `None` if nothing is saved in 5 s.
async fn wait_saved<F, Fut>(read: F) -> Option<GameId>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Option<GameId>>,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let saved = read().await;
        if saved.is_some() || Instant::now() > deadline {
            return saved;
        }
        sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn ssh_snake_opens_snake_and_nobody_is_called_snake() {
    let server = TestServer::start();
    let mut p = connect_as(server.addr, Login::GuestNamed("snake")).await;
    p.wait_for("Single player").await;
    let home = p.text();
    assert!(home.contains("Playing as ~player-"), "{home}");
    assert!(!home.contains("~snake"), "{home}");
}

#[tokio::test]
async fn ssh_chess_opens_what_this_server_has_until_chess_is_served() {
    let server = TestServer::start();
    let mut p = connect_as(server.addr, Login::GuestNamed("chess")).await;
    p.wait_for("Single player").await;
    let home = p.text();
    assert!(home.contains("Playing as ~player-"), "{home}");
    // With one game there's nothing to pick.
    assert!(
        !home.contains("Other games") && !home.contains("g games"),
        "{home}"
    );
}

#[tokio::test]
async fn the_game_open_is_kept_for_the_key_and_then_the_account() {
    let server = TestServer::start();
    let key = new_key();
    let fp = fingerprint(&key);
    let mut p = connect_as(server.addr, Login::Key(&key)).await;
    let db = store(&server).await;
    let guest = wait_saved(|| async { db.visit(&fp, "alice").await.unwrap().last_game }).await;
    assert_eq!(guest, Some(GameId::Snake));

    sign_up(&mut p, "alice").await;
    let account = wait_saved(|| async {
        let data = db.account_for_key(&fp).await.unwrap();
        data.and_then(|d| d.last_game)
    })
    .await;
    assert_eq!(account, Some(GameId::Snake));
}
