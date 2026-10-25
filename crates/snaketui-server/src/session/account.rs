//! A session's account: signing up, in and out, remembered keys, its
//! rating, friends, and invites.

use std::sync::Arc;
use std::time::Duration;

use actix::prelude::*;
use snake_core::config::GameConfig;
use snake_core::records::Records;
use snake_core::round::{JoinError, RoomKind};
use snake_tui::Screen;
use snake_tui::online::{
    FriendRow, FriendsView, KeyRow, LeaderRow, LeaderboardView, MyPlace, Profile, RANKED_AFTER,
    Secret,
};
use tracing::{debug, warn};

use super::SessionActor;
use crate::auth;
use crate::presence::{
    Deliver, Doing, Invite, Notice, Offline, Online, SetStatus, Status, StatusOf,
};
use crate::rating::Rating;
use crate::room::{Rated, RoomHandle};
use crate::store::{Account, AccountData, AccountId, Requested, now};

/// Wrong passwords one connection may try before it's disconnected.
const MAX_SIGN_IN_TRIES: u8 = 5;
/// Players the leaderboard lists.
const LEADERBOARD_SIZE: u32 = 20;
/// How long an invite waits for an answer.
const INVITE_LIFETIME: Duration = Duration::from_secs(120);
const TRY_AGAIN: &str = "Something went wrong. Try again.";
const TOO_MANY_TRIES: &str = "Too many wrong sign-ins. Goodbye.\r\n";

enum SignedUp {
    Made(Account),
    Taken,
    Refused(auth::Refused),
}

enum SignedIn {
    Yes(Box<AccountData>),
    Wrong,
    Refused(auth::Refused),
}

/// An invite this player hasn't answered yet.
pub(super) struct PendingInvite {
    invite: Invite,
    expires: SpawnHandle,
}

/// "today", "yesterday" or "N days ago", for a time in Unix seconds.
fn ago(then: i64) -> String {
    match (now() - then).max(0) / 86_400 {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        days => format!("{days} days ago"),
    }
}

fn profile(account: &Account) -> Profile {
    Profile {
        username: account.username.clone(),
        rating: account.rating.shown(),
        games: account.games,
        wins: account.wins,
    }
}

impl SessionActor {
    pub(super) fn account_id(&self) -> Option<AccountId> {
        self.account.as_ref().map(|a| a.id)
    }

    /// The rating this player is matched by: their account's, or the
    /// starting one for a guest.
    pub(super) fn rating(&self) -> Rating {
        self.account.as_ref().map(|a| a.rating).unwrap_or_default()
    }

    /// This session is now signed in to `account`.
    pub(super) fn became(&mut self, account: Account, ctx: &mut Context<Self>) {
        self.shared.presence.do_send(Online {
            account: account.id,
            session: self.id,
            notices: ctx.address().recipient(),
            status: Status::Menu,
        });
        self.status = Some(Doing {
            status: Status::Menu,
            room: None,
        });
        if let Some(app) = self.app.as_mut() {
            app.set_profile(Some(profile(&account)));
        }
        self.account = Some(account);
        self.refresh_friends(ctx);
    }

    /// Tells friends what this player is doing now, if that changed.
    pub(super) fn report_status(&mut self) {
        let (Some(account), Some(app)) = (self.account_id(), self.app.as_ref()) else {
            return;
        };
        let room = app.online.as_ref().and_then(|o| o.room.as_ref());
        let status = match (app.screen, room) {
            (_, Some(room)) if room.watching.is_some() => Status::Watching,
            (_, Some(room)) if room.view.kind == RoomKind::Match => Status::Match,
            (_, Some(_)) => Status::Room,
            (Screen::Finding, _) => Status::Queue,
            (
                Screen::Menu | Screen::Playing | Screen::Paused { .. } | Screen::GameOver { .. },
                _,
            ) => Status::SinglePlayer,
            _ => Status::Menu,
        };
        let doing = Doing {
            status,
            room: self.room.as_ref().map(|link| link.room.at()),
        };
        if self.status != Some(doing) {
            self.status = Some(doing);
            self.shared.presence.do_send(SetStatus {
                account,
                session: self.id,
                doing,
            });
        }
    }

    /// The session is ending: friends see the player go offline.
    pub(super) fn sign_off(&mut self) {
        if let Some(account) = self.account_id() {
            self.shared.presence.do_send(Offline {
                account,
                session: self.id,
            });
            let store = self.shared.store.clone();
            actix::spawn(async move {
                let _ = store.touch(account).await;
            });
        }
    }

    fn form_failed(&mut self, why: &str, ctx: &mut Context<Self>) {
        if let Some(app) = self.app.as_mut() {
            app.form_failed(why.to_string());
        }
        self.render(ctx);
    }

    pub(super) fn sign_up(&mut self, username: String, password: Secret, ctx: &mut Context<Self>) {
        let checked = auth::check_username(&username)
            .and_then(|()| auth::check_password(&username, password.expose()));
        if let Err(why) = checked {
            return self.form_failed(why, ctx);
        }
        let Some(app) = self.app.as_ref() else { return };
        let (cfg, records) = (app.cfg.clone(), app.records.clone());
        let (store, hasher) = (self.shared.store.clone(), self.shared.hasher.clone());
        let guesses = self.shared.guesses.clone();
        let key = self.fingerprint();
        let ip = self.peer.ip();
        let made = async move {
            let _attempt = match guesses.begin(&username, ip) {
                Ok(attempt) => attempt,
                Err(refused) => return anyhow::Ok(SignedUp::Refused(refused)),
            };
            if let Err(refused) = guesses.may_sign_up(ip, now()).await? {
                return Ok(SignedUp::Refused(refused));
            }
            // A taken name costs no hashing.
            if store.find_account(&username).await?.is_some() {
                return Ok(SignedUp::Taken);
            }
            let hash = hasher.hash(password.into_inner()).await?;
            let made = store
                .create_account(&username, &hash, key.as_deref(), &cfg, &records)
                .await?;
            let Some(account) = made else {
                return Ok(SignedUp::Taken);
            };
            guesses.signed_up(ip, now()).await?;
            Ok(SignedUp::Made(account))
        };
        ctx.spawn(made.into_actor(self).map(move |made, act, ctx| {
            match made {
                Ok(SignedUp::Made(account)) => {
                    debug!(peer = %act.peer, user = %account.username, "signed up");
                    let shown = profile(&account);
                    act.became(account, ctx);
                    if let Some(app) = act.app.as_mut() {
                        app.signed_up(shown);
                    }
                    act.report_status();
                }
                Ok(SignedUp::Taken) => return act.form_failed("That username is taken.", ctx),
                Ok(SignedUp::Refused(refused)) => return act.form_failed(refused.message(), ctx),
                Err(e) => {
                    warn!(error = %e, "couldn't make an account");
                    return act.form_failed(TRY_AGAIN, ctx);
                }
            }
            act.render(ctx);
        }));
    }

    pub(super) fn sign_in(
        &mut self,
        username: String,
        password: Secret,
        remember: bool,
        ctx: &mut Context<Self>,
    ) {
        let (store, hasher) = (self.shared.store.clone(), self.shared.hasher.clone());
        let guesses = self.shared.guesses.clone();
        let key = self.fingerprint().filter(|_| remember);
        let ip = self.peer.ip();
        let checked = async move {
            let _attempt = match guesses.begin(&username, ip) {
                Ok(attempt) => attempt,
                Err(refused) => return anyhow::Ok(SignedIn::Refused(refused)),
            };
            if let Err(refused) = guesses.may_sign_in(&username, ip, now()).await? {
                return Ok(SignedIn::Refused(refused));
            }
            let found = store.find_account(&username).await?;
            let (account, hash) = found.map_or((None, None), |(a, h)| (Some(a), Some(h)));
            let right = hasher.verify(password.into_inner(), hash).await;
            let Some(account) = account.filter(|_| right) else {
                guesses.failed(&username, ip, now()).await?;
                return Ok(SignedIn::Wrong);
            };
            guesses.succeeded(&username).await?;
            if let Some(key) = key {
                store.remember_key(account.id, &key).await?;
            }
            Ok(SignedIn::Yes(Box::new(
                store.load_account(account.id).await?,
            )))
        };
        ctx.spawn(checked.into_actor(self).map(move |checked, act, ctx| {
            match checked {
                Ok(SignedIn::Yes(data)) => {
                    let AccountData {
                        account,
                        cfg,
                        records,
                    } = *data;
                    debug!(peer = %act.peer, user = %account.username, "signed in");
                    let shown = profile(&account);
                    act.became(account, ctx);
                    if let Some(app) = act.app.as_mut() {
                        app.signed_in(shown, cfg, records);
                    }
                    act.report_status();
                }
                Ok(SignedIn::Wrong) => {
                    act.sign_in_failures += 1;
                    debug!(peer = %act.peer, tries = act.sign_in_failures, "wrong sign-in");
                    if act.sign_in_failures >= MAX_SIGN_IN_TRIES {
                        return act.close(1, Some(TOO_MANY_TRIES), ctx);
                    }
                    return act.form_failed("Wrong username or password.", ctx);
                }
                Ok(SignedIn::Refused(refused)) => return act.form_failed(refused.message(), ctx),
                Err(e) => {
                    warn!(error = %e, "couldn't check a sign-in");
                    return act.form_failed(TRY_AGAIN, ctx);
                }
            }
            act.render(ctx);
        }));
    }

    /// Signs out on this machine: forgets its key, and the player is a
    /// guest again.
    pub(super) fn sign_out(&mut self, ctx: &mut Context<Self>) {
        let Some(account) = self.account_id() else {
            return;
        };
        self.sign_off();
        self.account = None;
        self.status = None;
        self.invite = None;
        let store = self.shared.store.clone();
        let player = self.player.clone();
        let back = async move {
            let fp = player.as_ref().and_then(|p| p.fingerprint());
            match (fp, player.as_ref()) {
                (Some(fp), Some(p)) => {
                    store.forget_key(account, fp).await?;
                    let saved = store.visit(fp, &p.nick).await?;
                    anyhow::Ok((Some(saved.nick), saved.cfg, saved.records))
                }
                _ => Ok((None, GameConfig::default(), Records::default())),
            }
        };
        ctx.spawn(back.into_actor(self).map(|back, act, ctx| {
            let (nick, cfg, records) = back.unwrap_or_else(|e| {
                warn!(error = %e, "couldn't sign out cleanly");
                (None, GameConfig::default(), Records::default())
            });
            if let (Some(nick), Some(player)) = (nick, act.player.as_mut()) {
                player.nick = nick;
            }
            let name = act.guest_name();
            if let Some(app) = act.app.as_mut() {
                app.signed_out(name, cfg, records);
            }
            act.render(ctx);
        }));
    }

    pub(super) fn change_password(&mut self, password: Secret, ctx: &mut Context<Self>) {
        let Some(account) = self.account.as_ref() else {
            return;
        };
        if let Err(why) = auth::check_password(&account.username, password.expose()) {
            return self.form_failed(why, ctx);
        }
        let (store, hasher, id) = (
            self.shared.store.clone(),
            self.shared.hasher.clone(),
            account.id,
        );
        let changed = async move {
            let hash = hasher.hash(password.into_inner()).await?;
            store.set_password(id, &hash).await
        };
        ctx.spawn(changed.into_actor(self).map(|changed, act, ctx| {
            match changed {
                Ok(()) => {
                    if let Some(app) = act.app.as_mut() {
                        app.password_changed();
                    }
                }
                Err(e) => {
                    warn!(error = %e, "couldn't change a password");
                    return act.form_failed(TRY_AGAIN, ctx);
                }
            }
            act.render(ctx);
        }));
    }

    /// Forgets `key` for the account, or with `None` just reloads the list.
    pub(super) fn refresh_keys(&mut self, forget: Option<String>, ctx: &mut Context<Self>) {
        let Some(account) = self.account_id() else {
            return;
        };
        // Only reloads are coalesced; forgetting a key always happens.
        let reload = forget.is_none();
        if reload && !self.loads.keys.start() {
            return;
        }
        let store = self.shared.store.clone();
        let mine = self.fingerprint();
        let keys = async move {
            if let Some(key) = forget {
                store.forget_key(account, &key).await?;
            }
            store.keys(account).await
        };
        ctx.spawn(keys.into_actor(self).map(move |keys, act, ctx| {
            if reload && act.loads.keys.done() {
                act.refresh_keys(None, ctx);
            }
            let keys = keys.unwrap_or_else(|e| {
                warn!(error = %e, "couldn't list keys");
                Vec::new()
            });
            let rows = keys
                .into_iter()
                .map(|k| KeyRow {
                    this_machine: mine.as_deref() == Some(k.fingerprint.as_str()),
                    last_used: format!("used {}", ago(k.last_used)),
                    fingerprint: k.fingerprint,
                })
                .collect();
            if let Some(app) = act.app.as_mut() {
                app.set_keys(rows);
            }
            act.render(ctx);
        }));
    }

    /// Reloads the friends list, with who's online and what they're doing.
    pub(super) fn refresh_friends(&mut self, ctx: &mut Context<Self>) {
        let Some(account) = self.account_id() else {
            return;
        };
        if !self.loads.friends.start() {
            return;
        }
        self.friends_refresh += 1;
        let asked = self.friends_refresh;
        let (store, presence) = (self.shared.store.clone(), self.shared.presence.clone());
        let lists = async move {
            let lists = store.friend_lists(account).await?;
            let ids = lists.friends.iter().map(|f| f.id).collect();
            let online = presence.send(StatusOf(ids)).await?;
            let mut friends: Vec<(bool, FriendRow)> = lists
                .friends
                .into_iter()
                .map(|f| {
                    let doing = online.get(&f.id);
                    let row = FriendRow {
                        online: doing.is_some(),
                        status: doing.map_or_else(
                            || format!("last seen {}", ago(f.last_seen)),
                            |d| d.status.label().to_string(),
                        ),
                        watch: doing.is_some_and(|d| d.watchable().is_some()),
                        name: f.username,
                    };
                    (row.online, row)
                })
                .collect();
            // Online friends first, each group in name order.
            friends.sort_by_key(|(online, _)| !online);
            anyhow::Ok(FriendsView {
                friends: friends.into_iter().map(|(_, row)| row).collect(),
                incoming: lists.incoming.into_iter().map(|f| f.username).collect(),
                outgoing: lists.outgoing.into_iter().map(|f| f.username).collect(),
            })
        };
        ctx.spawn(lists.into_actor(self).map(move |lists, act, ctx| {
            if act.loads.friends.done() {
                act.refresh_friends(ctx);
            }
            // Refreshes overlap, and one that started earlier can finish
            // later, with a list from before: say, read halfway through
            // accepting a request, so it has neither the request nor the
            // new friend. Only the latest one counts.
            if asked != act.friends_refresh {
                return;
            }
            match lists {
                Ok(view) => {
                    if let Some(app) = act.app.as_mut() {
                        app.set_friends(view);
                    }
                }
                Err(e) => warn!(error = %e, "couldn't list friends"),
            }
            act.render(ctx);
        }));
    }

    /// Loads the best players, and this player's place among everyone.
    pub(super) fn load_leaderboard(&mut self, ctx: &mut Context<Self>) {
        if !self.loads.leaderboard.start() {
            return;
        }
        let (store, me) = (self.shared.store.clone(), self.account_id());
        let board = async move {
            let best = store.leaderboard(RANKED_AFTER, LEADERBOARD_SIZE).await?;
            let place = match me {
                Some(id) => store.place_of(id, RANKED_AFTER).await?,
                None => None,
            };
            anyhow::Ok((best, place))
        };
        ctx.spawn(board.into_actor(self).map(|board, act, ctx| {
            if act.loads.leaderboard.done() {
                act.load_leaderboard(ctx);
            }
            let (best, place) = match board {
                Ok(board) => board,
                Err(e) => {
                    warn!(error = %e, "couldn't load the leaderboard");
                    return act.notify(TRY_AGAIN.to_string(), ctx);
                }
            };
            let rows: Vec<LeaderRow> = best
                .into_iter()
                .zip(1..)
                .map(|(r, place)| LeaderRow {
                    place,
                    name: r.username,
                    rating: r.rating.shown(),
                    games: r.games,
                    wins: r.wins,
                })
                .collect();
            let me = match (&act.account, place) {
                (None, _) => MyPlace::Guest,
                // Listed, so it shows the same numbers as the list.
                (Some(account), Some(place)) => {
                    MyPlace::Ranked(rows.get(place as usize - 1).cloned().unwrap_or_else(|| {
                        LeaderRow {
                            place,
                            name: account.username.clone(),
                            rating: account.rating.shown(),
                            games: account.games,
                            wins: account.wins,
                        }
                    }))
                }
                // Saving a round's ratings can lag a moment behind the
                // session hearing about it.
                (Some(account), None) => MyPlace::Unranked {
                    games_left: RANKED_AFTER.saturating_sub(account.games).max(1),
                },
            };
            if let Some(app) = act.app.as_mut() {
                app.set_leaderboard(LeaderboardView { rows, me });
            }
            act.render(ctx);
        }));
    }

    fn notify(&mut self, message: String, ctx: &mut Context<Self>) {
        if let Some(app) = self.app.as_mut() {
            app.notify(message);
        }
        self.render(ctx);
    }

    pub(super) fn add_friend(&mut self, name: String, ctx: &mut Context<Self>) {
        let Some(me) = self.account.clone() else {
            return;
        };
        let (store, presence) = (self.shared.store.clone(), self.shared.presence.clone());
        let asked = async move {
            let Some((them, _)) = store.find_account(&name).await? else {
                return anyhow::Ok(format!("Nobody is called {name}."));
            };
            if them.id == me.id {
                return Ok("That's you.".to_string());
            }
            let who = them.username;
            Ok(match store.request_friend(me.id, them.id).await? {
                Requested::Sent => {
                    let notice = Notice::FriendRequest {
                        from: me.username.clone(),
                    };
                    presence.do_send(Deliver {
                        to: them.id,
                        notice,
                    });
                    format!("Asked {who} to be friends.")
                }
                Requested::NowFriends => {
                    let notice = Notice::NowFriends {
                        with: me.username.clone(),
                    };
                    presence.do_send(Deliver {
                        to: them.id,
                        notice,
                    });
                    format!("You and {who} are now friends.")
                }
                Requested::AlreadyFriends => format!("You're already friends with {who}."),
                Requested::AlreadyAsked => format!("You already asked {who}."),
                Requested::TooMany => "You have too many friends or open requests.".to_string(),
            })
        };
        ctx.spawn(asked.into_actor(self).map(|said, act, ctx| {
            let said = said.unwrap_or_else(|e| {
                warn!(error = %e, "couldn't send a friend request");
                TRY_AGAIN.to_string()
            });
            act.notify(said, ctx);
            act.refresh_friends(ctx);
        }));
    }

    pub(super) fn answer_friend(&mut self, name: String, accept: bool, ctx: &mut Context<Self>) {
        let Some(me) = self.account.clone() else {
            return;
        };
        let (store, presence) = (self.shared.store.clone(), self.shared.presence.clone());
        let answered = async move {
            let Some((them, _)) = store.find_account(&name).await? else {
                return anyhow::Ok(None);
            };
            if !accept {
                store.decline_friend(me.id, them.id).await?;
                return Ok(None);
            }
            if !store.accept_friend(me.id, them.id).await? {
                return Ok(Some("That request isn't there any more.".to_string()));
            }
            presence.do_send(Deliver {
                to: them.id,
                notice: Notice::NowFriends {
                    with: me.username.clone(),
                },
            });
            Ok(Some(format!("You and {} are now friends.", them.username)))
        };
        ctx.spawn(answered.into_actor(self).map(|said, act, ctx| {
            match said {
                Ok(Some(said)) => act.notify(said, ctx),
                Ok(None) => {}
                Err(e) => warn!(error = %e, "couldn't answer a friend request"),
            }
            act.refresh_friends(ctx);
        }));
    }

    pub(super) fn remove_friend(&mut self, name: String, ctx: &mut Context<Self>) {
        let Some(me) = self.account_id() else {
            return;
        };
        let store = self.shared.store.clone();
        let removed = async move {
            if let Some((them, _)) = store.find_account(&name).await? {
                store.unfriend(me, them.id).await?;
            }
            anyhow::Ok(())
        };
        ctx.spawn(removed.into_actor(self).map(|removed, act, ctx| {
            if let Err(e) = removed {
                warn!(error = %e, "couldn't remove a friend");
            }
            act.refresh_friends(ctx);
        }));
    }

    /// Watches the friend `name` play, in whatever room they're in, on
    /// whichever server.
    pub(super) fn watch(&mut self, name: String, ctx: &mut Context<Self>) {
        let Some(me) = self.account.clone() else {
            return;
        };
        let shared = Arc::clone(&self.shared);
        let who = self.entrant();
        let seat = Arc::downgrade(&self.presence);
        let found = async move {
            let Some((them, _)) = shared.store.find_account(&name).await? else {
                return anyhow::Ok(Err(format!("Nobody is called {name}.")));
            };
            if !shared.store.are_friends(me.id, them.id).await? {
                return Ok(Err("You can only watch friends.".to_string()));
            }
            let doing = shared.presence.send(StatusOf(vec![them.id])).await?;
            let over = format!("{} isn't playing a game to watch.", them.username);
            let Some(room) = doing
                .get(&them.id)
                .and_then(|d| d.watchable())
                .and_then(|at| shared.room(at))
            else {
                return Ok(Err(over));
            };
            Ok(match room.watch(who, seat).await {
                Ok(view) => Ok((room, view, them.username)),
                Err(JoinError::Full) => Err("Too many people are watching that game.".into()),
                Err(JoinError::Closed) => Err(over),
            })
        };
        ctx.spawn(found.into_actor(self).map(|found, act, ctx| match found {
            Ok(Ok((room, view, friend))) => act.watching(room, view, friend, ctx),
            Ok(Err(why)) => act.watch_failed(why, ctx),
            Err(e) => {
                warn!(error = %e, "couldn't find a game to watch");
                act.watch_failed(TRY_AGAIN.to_string(), ctx);
            }
        }));
    }

    fn watch_failed(&mut self, why: String, ctx: &mut Context<Self>) {
        if let Some(app) = self.app.as_mut() {
            app.watch_failed(why);
        }
        self.render(ctx);
    }

    /// Invites the friend `name` to the private room the player is in, or
    /// to a new one made with their settings.
    pub(super) fn invite(&mut self, name: String, ctx: &mut Context<Self>) {
        let (Some(me), Some(app)) = (self.account.clone(), self.app.as_ref()) else {
            return;
        };
        let in_private = app
            .online
            .as_ref()
            .and_then(|o| o.room.as_ref())
            .is_some_and(|r| r.view.kind == RoomKind::Private);
        let room = self
            .room
            .as_ref()
            .map(|link| link.room.at())
            .filter(|_| in_private);
        let rules = app.cfg.rules.clone();
        let (store, presence) = (self.shared.store.clone(), self.shared.presence.clone());
        let new_room = self.room_maker(rules);
        let invited = async move {
            let Some((them, _)) = store.find_account(&name).await? else {
                return anyhow::Ok(Err(format!("Nobody is called {name}.")));
            };
            if !store.are_friends(me.id, them.id).await? {
                return Ok(Err("You can only invite friends.".to_string()));
            }
            let online = presence.send(StatusOf(vec![them.id])).await?;
            match online.get(&them.id).map(|d| d.status) {
                None => return Ok(Err(format!("{} is offline.", them.username))),
                Some(Status::Match) => {
                    return Ok(Err(format!("{} is in a match right now.", them.username)));
                }
                Some(_) => {}
            }
            let (room, joined) = match room {
                Some(room) => (room, None),
                None => match new_room.await {
                    Ok((room, view)) => (room.at(), Some((room, view))),
                    Err(why) => return Ok(Err(why.message().to_string())),
                },
            };
            presence.do_send(Deliver {
                to: them.id,
                notice: Notice::Invite(Invite {
                    from: me.username.clone(),
                    room,
                }),
            });
            Ok(Ok((format!("Invited {}.", them.username), joined)))
        };
        ctx.spawn(
            invited
                .into_actor(self)
                .map(|invited, act, ctx| match invited {
                    Ok(Ok((said, joined))) => {
                        if let Some((room, view)) = joined {
                            act.entered(room, view, ctx);
                        }
                        act.notify(said, ctx);
                    }
                    Ok(Err(why)) => act.invite_failed(why, ctx),
                    Err(e) => {
                        warn!(error = %e, "couldn't send an invite");
                        act.invite_failed(TRY_AGAIN.to_string(), ctx);
                    }
                }),
        );
    }

    fn invite_failed(&mut self, why: String, ctx: &mut Context<Self>) {
        if let Some(app) = self.app.as_mut() {
            if matches!(app.screen, Screen::Joining { .. }) {
                app.join_failed(why);
            } else {
                app.notify(why);
            }
        }
        self.render(ctx);
    }

    pub(super) fn accept_invite(&mut self, ctx: &mut Context<Self>) {
        let Some(pending) = self.invite.take() else {
            if let Some(app) = self.app.as_mut() {
                app.join_failed("That invite has expired.".to_string());
            }
            return self.render(ctx);
        };
        ctx.cancel_future(pending.expires);
        let Some(room) = self.shared.room(pending.invite.room) else {
            return self.invite_failed("That room has closed.".into(), ctx);
        };
        let join = room.join(self.entrant(), std::sync::Arc::downgrade(&self.presence));
        ctx.spawn(
            join.into_actor(self)
                .map(move |joined, act, ctx| match joined {
                    Ok(view) => act.entered(room, view, ctx),
                    Err(JoinError::Full) => act.invite_failed("That room is full.".into(), ctx),
                    Err(JoinError::Closed) => {
                        act.invite_failed("That room has closed.".into(), ctx)
                    }
                }),
        );
    }

    pub(super) fn dismiss_invite(&mut self, ctx: &mut Context<Self>) {
        if let Some(pending) = self.invite.take() {
            ctx.cancel_future(pending.expires);
        }
    }

    fn invited(&mut self, invite: Invite, ctx: &mut Context<Self>) {
        self.dismiss_invite(ctx);
        let expires = ctx.run_later(INVITE_LIFETIME, |act, ctx| {
            act.invite = None;
            if let Some(app) = act.app.as_mut() {
                app.invite_gone();
            }
            act.render(ctx);
        });
        if let Some(app) = self.app.as_mut() {
            app.invited(invite.from.clone());
        }
        self.invite = Some(PendingInvite { invite, expires });
        self.render(ctx);
    }
}

impl Handler<Notice> for SessionActor {
    type Result = ();

    fn handle(&mut self, notice: Notice, ctx: &mut Self::Context) {
        if self.account.is_none() || self.closing {
            return;
        }
        match notice {
            Notice::FriendRequest { from } => {
                if let Some(app) = self.app.as_mut() {
                    app.friend_requested(&from);
                }
                self.refresh_friends(ctx);
            }
            Notice::NowFriends { with } => {
                self.notify(format!("You and {with} are now friends."), ctx);
                self.refresh_friends(ctx);
            }
            Notice::Invite(invite) => self.invited(invite, ctx),
        }
    }
}

impl Handler<Rated> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: Rated, ctx: &mut Self::Context) {
        if let Some(account) = self.account.as_mut() {
            account.rating = msg.rating;
            account.games += 1;
            account.wins += u32::from(msg.won);
            if let Some(app) = self.app.as_mut() {
                app.rated(msg.rating.shown(), msg.won);
            }
            self.render(ctx);
        }
    }
}

/// Room addresses and their views, for joining.
pub(super) type JoinedRoom = (RoomHandle, crate::room::ViewRx);
