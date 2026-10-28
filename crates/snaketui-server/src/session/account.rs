//! A session's account: signing up, in and out, remembered keys, its
//! rating, friends, and invites.

use std::sync::Arc;
use std::time::Duration;

use actix::prelude::*;
use game_api::Records;
use game_api::{GameId, JoinError, RatingKey, RoomKind};
use shell_tui::Screen;
use shell_tui::online::{
    FriendRow, FriendsView, KeyRow, LeaderRow, LeaderboardView, MyPlace, Profile, RANKED_AFTER,
    Secret, ShownRating,
};
use tracing::{debug, warn};

use super::{SessionActor, TRY_AGAIN};
use crate::auth;
use crate::games;
use crate::presence::{
    Deliver, Doing, Invite, Notice, Offline, Online, SetStatus, Status, StatusOf,
};
use crate::room::{Rated, RoomCmd, RoomHandle, RoomRef};
use crate::store::{Account, AccountData, AccountId, Requested, Standing, now};

/// Wrong passwords one connection may try before it's disconnected.
const MAX_SIGN_IN_TRIES: u8 = 5;
/// Players the leaderboard lists.
const LEADERBOARD_SIZE: u32 = 20;
/// How long an invite waits for an answer.
const INVITE_LIFETIME: Duration = Duration::from_secs(120);
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

/// Every rating of every game here.
fn every_rating() -> impl Iterator<Item = RatingKey> {
    games::GAMES.iter().flat_map(|g| {
        let info = g.info();
        info.categories.iter().map(move |category| RatingKey {
            game: info.id,
            category,
        })
    })
}

impl SessionActor {
    pub(super) fn account_id(&self) -> Option<AccountId> {
        self.account.as_ref().map(|a| a.id)
    }

    /// The account's standing in `key`, which it's matched by: a new
    /// player's for a guest, or in a rating it hasn't played.
    pub(super) fn standing(&self, key: RatingKey) -> Standing {
        self.ratings
            .get(&key)
            .copied()
            .unwrap_or_else(|| Standing::new(key))
    }

    /// The account as shown, with its rating in every game here.
    fn profile(&self, account: &Account) -> Profile {
        let ratings = every_rating()
            .map(|key| {
                let standing = self.standing(key);
                ShownRating {
                    key,
                    rating: standing.rating.shown(),
                    provisional: standing.is_provisional(now()),
                    games: standing.games,
                    wins: standing.wins,
                }
            })
            .collect();
        Profile {
            username: account.username.clone(),
            ratings,
        }
    }

    /// The game friends see this player has open.
    fn open_game(&self) -> GameId {
        self.app
            .as_ref()
            .and_then(|a| a.game())
            .unwrap_or(GameId::Snake)
    }

    /// This session is now signed in to `account`, with its standing in
    /// the `ratings` it has played.
    pub(super) fn became(
        &mut self,
        account: Account,
        ratings: Vec<(RatingKey, Standing)>,
        ctx: &mut Context<Self>,
    ) {
        let game = self.open_game();
        self.shared.presence.do_send(Online {
            account: account.id,
            session: self.id,
            notices: ctx.address().recipient(),
            status: Status::Menu,
            game,
        });
        self.status = Some(Doing {
            status: Status::Menu,
            game,
            room: None,
        });
        self.ratings = ratings.into_iter().collect();
        let shown = self.profile(&account);
        if let Some(app) = self.app.as_mut() {
            app.set_profile(Some(shown));
        }
        self.account = Some(account);
        self.refresh_friends(ctx);
        self.rejoin(ctx);
    }

    /// Takes the player back to a game they're still seated in, from a
    /// connection that dropped or that they left mid-game, if they're free
    /// to go: not in a room, the queue or a game of their own.
    fn rejoin(&mut self, ctx: &mut Context<Self>) {
        let Some(account) = self.account_id() else {
            return;
        };
        let free = |act: &Self| {
            !act.closing
                && act.room.is_none()
                && act.queued.is_none()
                && act.app.as_ref().is_some_and(|a| {
                    !a.in_room() && !a.playing_alone() && a.screen != Screen::Finding
                })
        };
        if !free(self) {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let who = self.entrant();
        let presence = Arc::downgrade(&self.presence);
        let on_rated = ctx.address().recipient();
        let back = async move {
            let at = match shared.rooms.seat_of(account) {
                Some(room) => RoomRef {
                    server: shared.server,
                    room,
                },
                None => shared.cluster.as_ref()?.seat_of(account).await?,
            };
            let room = shared.room(at)?;
            let view = room.rejoin(who, account, presence, on_rated).await.ok()?;
            Some((room, view))
        };
        ctx.spawn(back.into_actor(self).map(move |back, act, ctx| {
            let Some((room, view)) = back else { return };
            let current = view.borrow().clone();
            let back_in = free(act)
                && act
                    .app
                    .as_mut()
                    .is_some_and(|app| app.rejoin_room(act.id, current));
            if !back_in {
                // Busy after all: the seat waits for them again.
                room.send(act.id, RoomCmd::Gone);
                return;
            }
            debug!(peer = %act.peer, "back in a game still being played");
            act.link(room, view, ctx);
        }));
    }

    /// Tells friends what this player is doing now, if that changed.
    pub(super) fn report_status(&mut self) {
        let (Some(account), Some(app)) = (self.account_id(), self.app.as_ref()) else {
            return;
        };
        let room = app.online.room.as_ref();
        let status = match (app.screen, room) {
            (_, Some(room)) if room.watching.is_some() => Status::Watching,
            (_, Some(room)) if room.shell().kind == RoomKind::Match => Status::Match,
            (_, Some(_)) => Status::Room,
            (Screen::Finding, _) => Status::Queue,
            // A game's own screens, outside rooms, are a game alone.
            (Screen::Game, _) => Status::SinglePlayer,
            _ => Status::Menu,
        };
        let doing = Doing {
            status,
            game: self.open_game(),
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
        // What's waiting to be saved is the guest's, not the account's.
        self.save_now();
        let checked = auth::check_username(&username)
            .and_then(|()| auth::check_password(&username, password.expose()));
        if let Err(why) = checked {
            return self.form_failed(why, ctx);
        }
        let Some(app) = self.app.as_ref() else { return };
        // The guest's settings come along, and Snake's records.
        let prefs: Vec<(GameId, String)> = games::GAMES
            .iter()
            .filter_map(|g| Some((g.info().id, app.saved(g.info().id)?.prefs)))
            .collect();
        let records = app.saved(GameId::Snake).unwrap_or_default().records;
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
                .create_account(&username, &hash, key.as_deref(), &prefs, &records)
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
                    let shown = act.profile(&account);
                    act.became(account, Vec::new(), ctx);
                    if let Some(app) = act.app.as_mut() {
                        app.signed_up(shown);
                    }
                    // A new account has no last game yet.
                    act.last_game = None;
                    act.note_game(ctx);
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
        // What's waiting to be saved is the guest's, not the account's.
        self.save_now();
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
                        ratings,
                        prefs,
                        records,
                        last_game,
                    } = *data;
                    debug!(peer = %act.peer, user = %account.username, "signed in");
                    act.became(account.clone(), ratings, ctx);
                    let shown = act.profile(&account);
                    if let Some(app) = act.app.as_mut() {
                        super::restore(app, prefs, records);
                        app.signed_in(shown);
                    }
                    // The game open now is the account's last one.
                    act.last_game = last_game;
                    act.note_game(ctx);
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
        // What's waiting to be saved is the account's, not the guest's.
        self.save_now();
        self.sign_off();
        self.account = None;
        self.ratings.clear();
        self.status = None;
        self.invite = None;
        let store = self.shared.store.clone();
        let player = self.player.clone();
        let back = async move {
            let fp = player.as_ref().and_then(|p| p.fingerprint());
            match (fp, player.as_ref()) {
                (Some(fp), Some(p)) => {
                    store.forget_key(account, fp).await?;
                    anyhow::Ok(Some(store.visit(fp, &p.nick).await?))
                }
                _ => Ok(None),
            }
        };
        ctx.spawn(back.into_actor(self).map(|back, act, ctx| {
            let saved = back.unwrap_or_else(|e| {
                warn!(error = %e, "couldn't sign out cleanly");
                None
            });
            let (prefs, records, last_game) = match saved {
                Some(saved) => {
                    if let Some(player) = act.player.as_mut() {
                        player.nick = saved.nick;
                    }
                    (saved.prefs, saved.records, saved.last_game)
                }
                None => (Vec::new(), Records::default(), None),
            };
            let name = act.guest_name();
            if let Some(app) = act.app.as_mut() {
                super::restore(app, prefs, records);
                app.signed_out(name);
            }
            act.last_game = last_game;
            act.note_game(ctx);
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

    /// Loads the best players by `key`, and this player's place among
    /// everyone.
    pub(super) fn load_leaderboard(&mut self, key: RatingKey, ctx: &mut Context<Self>) {
        // The one asked for last is the one loaded next.
        self.loads.leaderboard_key = Some(key);
        if !self.loads.leaderboard.start() {
            return;
        }
        let (store, me) = (self.shared.store.clone(), self.account_id());
        let board = async move {
            let best = store
                .leaderboard(key, RANKED_AFTER, LEADERBOARD_SIZE)
                .await?;
            let place = match me {
                Some(id) => store.place_of(key, id, RANKED_AFTER).await?,
                None => None,
            };
            anyhow::Ok((best, place))
        };
        ctx.spawn(board.into_actor(self).map(move |board, act, ctx| {
            let latest = act.loads.leaderboard_key.unwrap_or(key);
            if act.loads.leaderboard.done() {
                act.load_leaderboard(latest, ctx);
            }
            // Another rating's board is on its way.
            if latest != key {
                return;
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
                    rating: r.standing.rating.shown(),
                    games: r.standing.games,
                    wins: r.standing.wins,
                    provisional: r.standing.is_provisional(now()),
                })
                .collect();
            let mine = act.standing(key);
            let me = match (&act.account, place) {
                (None, _) => MyPlace::Guest,
                // Listed, so it shows the same numbers as the list.
                (Some(account), Some(place)) => {
                    MyPlace::Ranked(rows.get(place as usize - 1).cloned().unwrap_or_else(|| {
                        LeaderRow {
                            place,
                            name: account.username.clone(),
                            rating: mine.rating.shown(),
                            games: mine.games,
                            wins: mine.wins,
                            provisional: mine.is_provisional(now()),
                        }
                    }))
                }
                // Saving a round's ratings can lag a moment behind the
                // session hearing about it.
                (Some(_), None) => MyPlace::Unranked {
                    games_left: RANKED_AFTER.saturating_sub(mine.games).max(1),
                },
            };
            if let Some(app) = act.app.as_mut() {
                app.set_leaderboard(LeaderboardView { key, rows, me });
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
        let guesses = self.shared.guesses.clone();
        let asked = async move {
            let Some((them, _)) = store.find_account(&name).await? else {
                return anyhow::Ok(format!("Nobody is called {name}."));
            };
            if them.id == me.id {
                return Ok("That's you.".to_string());
            }
            if let Err(refused) = guesses.may_ask_friend(me.id, them.id, now()).await? {
                return Ok(refused.message().to_string());
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
            .room
            .as_ref()
            .is_some_and(|r| r.shell().kind == RoomKind::Private);
        let room = self
            .room
            .as_ref()
            .map(|link| (link.room.at(), link.game))
            .filter(|_| in_private);
        let (game, settings) = app.room_to_make();
        let (store, presence) = (self.shared.store.clone(), self.shared.presence.clone());
        let guesses = self.shared.guesses.clone();
        let new_room = self.room_maker(game, &settings);
        let invited = async move {
            let Some((them, _)) = store.find_account(&name).await? else {
                return anyhow::Ok(Err(format!("Nobody is called {name}.")));
            };
            if !store.are_friends(me.id, them.id).await? {
                return Ok(Err("You can only invite friends.".to_string()));
            }
            if let Err(refused) = guesses.may_invite(me.id, them.id, now()).await? {
                return Ok(Err(refused.message().to_string()));
            }
            let online = presence.send(StatusOf(vec![them.id])).await?;
            match online.get(&them.id).map(|d| d.status) {
                None => return Ok(Err(format!("{} is offline.", them.username))),
                Some(Status::Match) => {
                    return Ok(Err(format!("{} is in a match right now.", them.username)));
                }
                Some(_) => {}
            }
            let ((room, game), joined) = match room {
                Some(room) => (room, None),
                None => match new_room.await {
                    Ok((room, view)) => ((room.at(), game), Some((room, view))),
                    Err(why) => return Ok(Err(why.message().to_string())),
                },
            };
            presence.do_send(Deliver {
                to: them.id,
                notice: Notice::Invite(Invite {
                    from: me.username.clone(),
                    game,
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
        let presence = std::sync::Arc::downgrade(&self.presence);
        let join = room.join(self.entrant(), self.account_id(), presence);
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
            app.invited(invite.from.clone(), invite.game);
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
        if self.account.is_none() {
            return;
        }
        let standing = self
            .ratings
            .entry(msg.key)
            .or_insert_with(|| Standing::new(msg.key));
        standing.rating = msg.rating;
        standing.games += 1;
        standing.wins += u32::from(msg.won);
        standing.rated_at = now();
        let provisional = standing.is_provisional(standing.rated_at);
        if let Some(app) = self.app.as_mut() {
            app.rated(msg.key, msg.rating.shown(), provisional, msg.won);
        }
        self.render(ctx);
    }
}

/// Room addresses and their views, for joining.
pub(super) type JoinedRoom = (RoomHandle, crate::room::ViewRx);
