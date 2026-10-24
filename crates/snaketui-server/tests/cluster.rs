//! Several servers sharing one Postgres database: accounts, remembered keys
//! and the guessing limits are the same on all of them.

mod common;

use std::time::Duration;

use common::accounts::*;
use common::*;

async fn two_servers() -> (TestServer, TestServer) {
    let db = snaketui_test_services::postgres().await;
    let a = TestServer::with(|c| c.database = db.clone());
    let b = TestServer::with(|c| c.database = db.clone());
    (a, b)
}

#[tokio::test]
async fn an_account_made_on_one_server_signs_in_on_another() {
    let (a, b) = two_servers().await;
    let (p, key) = signed_up(&a, "ann").await;
    drop(p);
    let on_b = connect_as(&b, Login::Key(&key)).await;
    assert!(on_b.text().contains("Signed in as ann"), "{}", on_b.text());
    drop(on_b);

    // A new machine signs in on B with the password; A knows it after.
    let other = new_key();
    let mut p = connect_as(&b, Login::Key(&other)).await;
    try_sign_in(&mut p, "ann", PASSWORD, true).await;
    p.wait_for("Signed in as ann").await;
    drop(p);
    let back_on_a = connect_as(&a, Login::Key(&other)).await;
    assert!(
        back_on_a.text().contains("Signed in as ann"),
        "{}",
        back_on_a.text()
    );
}

#[tokio::test]
async fn a_username_locked_on_one_server_is_locked_on_all() {
    let (a, b) = two_servers().await;
    let _ann = signed_up(&a, "ann").await;
    let mut p = connect_as(&a, Login::Guest).await;
    try_sign_in(&mut p, "ann", "wrong guess 1", false).await;
    p.wait_for(WRONG).await;
    for i in 2..=4 {
        retry_sign_in(&mut p, "ann", &format!("wrong guess {i}")).await;
        p.wait_for(WRONG).await;
    }
    retry_sign_in(&mut p, "ann", "wrong guess 5").await;
    p.wait_closed(Duration::from_secs(5)).await;

    // Even the right password, on the other server, has to wait.
    let mut q = connect_as(&b, Login::Guest).await;
    try_sign_in(&mut q, "ann", PASSWORD, false).await;
    q.wait_for("Too many wrong tries for that name").await;
}
