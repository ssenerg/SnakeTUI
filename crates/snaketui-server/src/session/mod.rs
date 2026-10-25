//! One player's connection: their screen, their keys and their game.

mod account;

use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use actix::prelude::*;
use bytes::Bytes;
use ratatui::layout::{Rect, Size};
use ratatui::{Terminal, TerminalOptions, Viewport};
use russh::ChannelId;
use russh::server::Handle;
use snake_core::config::{GameConfig, Rules};
use snake_core::records::Records;
use snake_core::round::{Entrant, JoinError, PhaseView, PlayerId};
use snake_tui::online::QueueRegion;
use snake_tui::{App, Effect, Key, Screen, ui};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use self::account::{JoinedRoom, PendingInvite};
use crate::auth::Refused;
use crate::backend::SshBackend;
use crate::identity::{Player, nickname};
use crate::input::{Budget, KeyParser, STATUS_QUERY};
use crate::lobby::{CreateError, CreateRoom, FindRoom};
use crate::matcher::REGION_WAIT;
use crate::matchmaker::{BotMatch, Dequeue, Enqueue, Matched};
use crate::metrics::PINGS;
use crate::presence::Doing;
use crate::room::{RESULTS_GRACE, RoomHandle, ViewRx};
use crate::store::{Account, AccountData, now};
use crate::{Shared, Stopping};

/// Largest terminal served. Bigger requests are clamped, so a client can't
/// make the server allocate huge screens.
pub const MAX_COLS: u16 = 240;
pub const MAX_ROWS: u16 = 80;
/// Frames waiting to go out before drawing pauses for a slow client.
const OUT_QUEUE: usize = 4;
/// How long a lone ESC waits to see whether it starts an arrow key.
const ESC_WAIT: Duration = Duration::from_millis(50);
/// While a game runs, wake at least this often for the clock and food.
const MAX_WAKE: Duration = Duration::from_millis(100);
/// How often the queue screen updates its clock.
const QUEUE_WAKE: Duration = Duration::from_secs(1);
/// How soon to retry drawing for a client that's behind.
const RETRY_DRAW: Duration = Duration::from_millis(50);
const IDLE_CHECK: Duration = Duration::from_secs(15);
/// How often the terminal is asked for a status report, to time the round
/// trip.
const PING_EVERY: Duration = Duration::from_secs(2);
/// An answer this late counts as none.
const PING_WAIT: Duration = Duration::from_secs(10);
/// Unanswered queries in a row before giving up: the client doesn't answer.
const PING_MISSES: u8 = 3;
/// After the game closes the channel, how long a client gets to hang up.
const HANG_UP_AFTER: Duration = Duration::from_secs(2);
/// Keys and window changes waiting in the mailbox; more are dropped.
const MAILBOX: usize = 64;
/// Wrong room codes a connection may try. After that it can't join by code.
const MAX_WRONG_CODES: u8 = 5;
/// Keys a player may send at once, enough to paste a password, and then
/// how many a second: faster than anyone types. A flood beyond is dropped.
const KEY_BURST: u32 = 256;
const KEYS_PER_SEC: u32 = 50;
/// Actions that reach the database or the lobby (signing in, friends,
/// rooms): a few at once, then a couple a second.
const ACTION_BURST: u32 = 10;
const ACTIONS_PER_SEC: u32 = 2;
const TOO_FAST: &str = "Slow down a little, then try again.";
/// Settings and nicknames are saved this long after their last change.
const SAVE_AFTER: Duration = Duration::from_millis(500);
/// Frames go out at most this often, however fast keys come.
const FRAME: Duration = Duration::from_millis(15);
/// The only region until there are several servers.
const LOCAL_REGION: &str = "local";

/// Switch to the alternate screen and hide the cursor; and undo both.
const ENTER_SCREEN: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H";
const LEAVE_SCREEN: &[u8] = b"\x1b[0m\x1b[?25h\x1b[?1049l";

pub const NEEDS_PTY: &str =
    "SnakeTUI is a game and needs a terminal. Connect without -T, or with: ssh -t <host>\r\n";
pub const NO_COMMANDS: &str =
    "This server only runs SnakeTUI and can't run commands. Connect with: ssh <host>\r\n";
const SHUTTING_DOWN: &str = "The SnakeTUI server is restarting. Please reconnect in a minute.\r\n";

/// What the writer task sends down the channel, in order.
enum Out {
    Data(Bytes),
    /// Report the exit status, close the channel, and drop the connection.
    Close {
        exit_status: u32,
    },
}

/// Cuts a connection at its socket. This works even when the SSH session
/// is stuck behind a client that stopped reading, where a polite SSH
/// disconnect would queue up behind the frames it isn't taking.
#[derive(Clone)]
pub struct Hangup(Arc<std::net::TcpStream>);

impl Hangup {
    /// Keeps a duplicate of `socket` to shut down later.
    pub fn new(socket: &std::net::TcpStream) -> std::io::Result<Self> {
        socket.try_clone().map(|s| Self(Arc::new(s)))
    }

    pub fn now(&self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

/// The SSH handshake is done; `Hangup` can end the connection.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Attach(pub Hangup);

#[derive(Message)]
#[rtype(result = "()")]
pub struct Authenticated(pub Player);

#[derive(Message)]
#[rtype(result = "()")]
pub struct ChannelOpened {
    pub channel: ChannelId,
    pub handle: Handle,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PtyRequested {
    pub cols: u32,
    pub rows: u32,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct ShellRequested;

/// The client asked for something other than the game: say why not, and
/// close.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Refuse(pub &'static str);

#[derive(Message)]
#[rtype(result = "()")]
pub struct Input(pub Bytes);

#[derive(Message)]
#[rtype(result = "()")]
pub struct Resized {
    pub cols: u32,
    pub rows: u32,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Disconnected;

/// Whether `effect` reaches the database or the lobby, and so spends one
/// of the player's actions.
fn costly(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::SignUp { .. }
            | Effect::SignIn { .. }
            | Effect::SignOut
            | Effect::ChangePassword(_)
            | Effect::ForgetKey(_)
            | Effect::AddFriend(_)
            | Effect::AnswerFriend { .. }
            | Effect::RemoveFriend(_)
            | Effect::Invite(_)
            | Effect::AcceptInvite
            | Effect::Watch(_)
            | Effect::CreateRoom(_)
            | Effect::JoinRoom(_)
            | Effect::PlayBots
    )
}

/// A load from the store: one at a time, and once more if it was asked
/// for again meanwhile, so a flood of requests makes one or two queries.
#[derive(Default)]
pub(super) struct Load {
    running: bool,
    again: bool,
}

impl Load {
    /// Whether to start one now. While one runs, it's asked for again.
    pub(super) fn start(&mut self) -> bool {
        if self.running {
            self.again = true;
            return false;
        }
        self.running = true;
        true
    }

    /// It finished: whether to run it once more.
    pub(super) fn done(&mut self) -> bool {
        self.running = false;
        std::mem::take(&mut self.again)
    }
}

#[derive(Default)]
pub(super) struct Loads {
    pub(super) friends: Load,
    pub(super) leaderboard: Load,
    pub(super) keys: Load,
}

/// What has changed since it was last saved.
#[derive(Default)]
struct Unsaved {
    config: bool,
    nick: bool,
}

/// The room a session's player is in.
struct RoomLink {
    room: RoomHandle,
    /// The future waiting for the room's next view.
    watch: SpawnHandle,
    /// Tells apart this stay from an earlier one in the same room.
    stay: u64,
}

/// Why a private room couldn't be made or joined.
enum JoinFail {
    NoSuchCode,
    TooManyCodes,
    CodesFromAddress,
    BoardTooSmall,
    Full,
    Closed,
    Unavailable,
}

impl JoinFail {
    fn message(&self) -> &'static str {
        match self {
            JoinFail::NoSuchCode => "No room has that code.",
            JoinFail::TooManyCodes => "Too many wrong codes. Reconnect to try again.",
            JoinFail::CodesFromAddress => Refused::TooManyCodes.message(),
            JoinFail::BoardTooSmall => {
                "Your board is too small for two snakes. Make it bigger in Single player."
            }
            JoinFail::Full => "That room is full.",
            JoinFail::Closed => "That room has closed.",
            JoinFail::Unavailable => "Couldn't make a room just now. Try again.",
        }
    }
}

pub struct SessionActor {
    shared: Arc<Shared>,
    peer: SocketAddr,
    id: PlayerId,
    /// Rooms and the queue hold a weak reference, to notice the session
    /// is gone even if it never said goodbye.
    presence: Arc<()>,
    room: Option<RoomLink>,
    stays: u64,
    queued: bool,
    wrong_codes: u8,
    /// Signed in to this account.
    account: Option<Account>,
    sign_in_failures: u8,
    invite: Option<PendingInvite>,
    /// Counts friends-list refreshes; only the latest one's list is shown.
    friends_refresh: u64,
    /// What friends were last told this player is doing.
    status: Option<Doing>,
    hangup: Option<Hangup>,
    out: Option<mpsc::Sender<Out>>,
    player: Option<Player>,
    size: Size,
    /// Both exist from when the game screen opens.
    terminal: Option<Terminal<SshBackend>>,
    app: Option<App>,
    parser: KeyParser,
    /// Keys this player may still send.
    keys: Budget,
    /// Actions this player may still take.
    actions: Budget,
    loads: Loads,
    unsaved: Unsaved,
    save_timer: Option<SpawnHandle>,
    /// When the last frame went out, and the next one waiting its turn.
    drawn_at: Option<Instant>,
    redraw: Option<SpawnHandle>,
    last_tick: Instant,
    last_input: Instant,
    /// The round trip to the player's terminal, smoothed.
    ping: Option<Duration>,
    /// When the status query waiting for an answer went out.
    ping_sent: Option<Instant>,
    ping_misses: u8,
    wake: Option<SpawnHandle>,
    esc_timer: Option<SpawnHandle>,
    retry: Option<SpawnHandle>,
    stalled_since: Option<Instant>,
    shell_requested: bool,
    closing: bool,
    /// The server is stopping: this player goes once they're not in a
    /// round.
    draining: bool,
    leaving: Option<SpawnHandle>,
}

impl Actor for SessionActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(MAILBOX);
        ctx.run_interval(IDLE_CHECK, |act, ctx| act.check_idle(ctx));
        let mut stopping = self.shared.shutdown.subscribe();
        ctx.spawn(
            async move {
                let _ = stopping.wait_for(|s| *s >= Stopping::Draining).await;
                stopping
            }
            .into_actor(self)
            .map(|mut stopping, act, ctx| {
                act.draining = true;
                act.leave_if_free(ctx);
                let now = async move {
                    let _ = stopping.wait_for(|s| *s == Stopping::Now).await;
                };
                ctx.spawn(now.into_actor(act).map(|_, act, ctx| act.goodbye(ctx)));
            }),
        );
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        debug!(peer = %self.peer, "session ended");
        self.save_now();
        self.sign_off();
        if let Some(link) = self.room.take() {
            link.room.leave(self.id);
        }
        if self.queued {
            self.shared.matchmaker.do_send(Dequeue(self.id));
        }
    }
}

impl SessionActor {
    pub fn new(shared: Arc<Shared>, peer: SocketAddr) -> Self {
        let now = Instant::now();
        let id = shared.next_player.fetch_add(1, Ordering::Relaxed);
        Self {
            shared,
            peer,
            id,
            presence: Arc::new(()),
            room: None,
            stays: 0,
            queued: false,
            wrong_codes: 0,
            account: None,
            sign_in_failures: 0,
            invite: None,
            friends_refresh: 0,
            status: None,
            hangup: None,
            out: None,
            player: None,
            size: Size::new(80, 24),
            terminal: None,
            app: None,
            parser: KeyParser::default(),
            keys: Budget::new(KEY_BURST, KEYS_PER_SEC, now),
            actions: Budget::new(ACTION_BURST, ACTIONS_PER_SEC, now),
            loads: Loads::default(),
            unsaved: Unsaved::default(),
            save_timer: None,
            drawn_at: None,
            redraw: None,
            last_tick: now,
            last_input: now,
            ping: None,
            ping_sent: None,
            ping_misses: 0,
            wake: None,
            esc_timer: None,
            retry: None,
            stalled_since: None,
            shell_requested: false,
            closing: false,
            draining: false,
            leaving: None,
        }
    }

    /// A pty size the server is willing to draw: 0 means unknown (80×24),
    /// and anything beyond [`MAX_COLS`]×[`MAX_ROWS`] is cut down.
    pub fn clamp_size(cols: u32, rows: u32) -> Size {
        let fit = |v: u32, max: u16, unknown: u16| {
            if v == 0 {
                unknown
            } else {
                v.min(u32::from(max)) as u16
            }
        };
        Size::new(fit(cols, MAX_COLS, 80), fit(rows, MAX_ROWS, 24))
    }

    fn send(&self, out: Out) -> bool {
        self.out.as_ref().is_some_and(|tx| tx.try_send(out).is_ok())
    }

    fn fingerprint(&self) -> Option<String> {
        self.player.as_ref()?.fingerprint().map(str::to_string)
    }

    /// Loads a returning player's account, or their settings and records as
    /// a guest, then opens the game.
    fn start(&mut self, ctx: &mut Context<Self>) {
        let store = self.shared.store.clone();
        let player = self.player.clone();
        let load = async move {
            let Some((fp, p)) = player.as_ref().and_then(|p| Some((p.fingerprint()?, p))) else {
                return anyhow::Ok(Loaded::Guest(
                    None,
                    GameConfig::default(),
                    Records::default(),
                ));
            };
            if let Some(data) = store.account_for_key(fp).await? {
                return Ok(Loaded::Account(data));
            }
            let saved = store.visit(fp, &p.nick).await?;
            Ok(Loaded::Guest(Some(saved.nick), saved.cfg, saved.records))
        };
        // Keys that arrive meanwhile wait in the mailbox.
        ctx.wait(load.into_actor(self).map(|loaded, act, ctx| {
            let loaded = loaded.unwrap_or_else(|e| {
                warn!(peer = %act.peer, error = %e, "couldn't load the player; starting fresh");
                Loaded::Guest(None, GameConfig::default(), Records::default())
            });
            match loaded {
                Loaded::Account(AccountData {
                    account,
                    cfg,
                    records,
                }) => {
                    debug!(peer = %act.peer, user = %account.username, "signed in by key");
                    act.open_screen(cfg, records, ctx);
                    act.became(account, ctx);
                }
                Loaded::Guest(nick, cfg, records) => {
                    if let (Some(nick), Some(player)) = (nick, act.player.as_mut()) {
                        player.nick = nick;
                    }
                    act.open_screen(cfg, records, ctx);
                }
            }
            act.render(ctx);
        }));
    }

    /// Sets up the terminal and the game; the caller draws the first frame.
    fn open_screen(&mut self, cfg: GameConfig, records: Records, ctx: &mut Context<Self>) {
        let viewport = Viewport::Fixed(Rect::new(0, 0, self.size.width, self.size.height));
        let terminal = match Terminal::with_options(
            SshBackend::new(self.size),
            TerminalOptions { viewport },
        ) {
            Ok(terminal) => terminal,
            Err(e) => {
                warn!(peer = %self.peer, error = %e, "couldn't set up the screen");
                return self.close(1, None, ctx);
            }
        };
        let mut app = App::online(
            cfg,
            records,
            self.guest_name(),
            self.fingerprint().is_some(),
        );
        if let Some(cluster) = &self.shared.cluster {
            app.set_region(QueueRegion {
                name: cluster.region.clone(),
                widens_after: REGION_WAIT,
            });
        }
        app.resize(self.size);
        self.app = Some(app);
        self.terminal = Some(terminal);
        self.last_input = Instant::now();
        self.send(Out::Data(Bytes::from_static(ENTER_SCREEN)));
        ctx.run_interval(PING_EVERY, |act, _| act.ping());
    }

    /// Asks the terminal for a status report: the wait for its answer is the
    /// round trip.
    fn ping(&mut self) {
        if self.closing || self.ping_misses >= PING_MISSES {
            return;
        }
        if let Some(sent) = self.ping_sent {
            if sent.elapsed() < PING_WAIT {
                return;
            }
            self.ping_sent = None;
            self.ping_misses += 1;
            if self.ping_misses == PING_MISSES {
                debug!(peer = %self.peer, "no answers to status queries; not timing round trips");
                return;
            }
        }
        if self.send(Out::Data(Bytes::from_static(STATUS_QUERY))) {
            self.ping_sent = Some(Instant::now());
        }
    }

    /// The terminal answered: one more round trip to go by.
    fn on_pong(&mut self) {
        let Some(sent) = self.ping_sent.take() else {
            return;
        };
        let rtt = sent.elapsed();
        self.ping_misses = 0;
        PINGS.record(rtt);
        // Smoothed like TCP's: each new sample counts for a quarter.
        let ping = self.ping.map_or(rtt, |old| (old * 3 + rtt) / 4);
        self.ping = Some(ping);
        if let Some(link) = &self.room {
            link.room.ping(self.id, ping);
        }
    }

    fn on_keys(&mut self, keys: Vec<Key>, ctx: &mut Context<Self>) {
        let Some(app) = self.app.as_mut() else { return };
        let was_playing = matches!(app.screen, Screen::Playing | Screen::Finding);
        let mut effects = Vec::new();
        for key in keys {
            let fx = app.handle(key);
            let quit = fx.contains(&Effect::Quit);
            effects.extend(fx);
            if quit {
                break;
            }
        }
        if !was_playing {
            // Time spent in menus or paused doesn't count toward the game.
            self.last_tick = Instant::now();
        }
        self.apply(effects, ctx);
        self.report_status();
        self.render(ctx);
        self.schedule_wake(ctx);
        self.leave_if_free(ctx);
    }

    fn apply(&mut self, effects: Vec<Effect>, ctx: &mut Context<Self>) {
        let fingerprint = self.fingerprint();
        let account = self.account_id();
        let store = self.shared.store.clone();
        for effect in effects {
            if costly(&effect) && self.actions.take(1, Instant::now()) == 0 {
                self.too_fast(ctx);
                continue;
            }
            match effect {
                Effect::SaveConfig => {
                    self.unsaved.config = true;
                    self.save_soon(ctx);
                }
                Effect::RecordSet { key, best } => {
                    let (store, fp) = (store.clone(), fingerprint.clone());
                    actix::spawn(async move {
                        let saved = match (account, fp) {
                            (Some(id), _) => store.save_account_record(id, &key, best).await,
                            (None, Some(fp)) => store.save_record(&fp, &key, best).await,
                            (None, None) => Ok(()),
                        };
                        if let Err(e) = saved {
                            warn!(error = %e, "couldn't save a record");
                        }
                    });
                }
                Effect::Quit => self.close(0, None, ctx),
                Effect::QuickPlay => self.enqueue(ctx),
                Effect::LeaveQueue => {
                    self.queued = false;
                    self.shared.matchmaker.do_send(Dequeue(self.id));
                }
                Effect::CreateRoom(rules) => self.join_private(Some(rules), None, ctx),
                Effect::JoinRoom(code) => self.join_private(None, Some(code), ctx),
                Effect::Steer(dir) => {
                    if let Some(link) = &self.room {
                        link.room.steer(self.id, dir);
                    }
                }
                Effect::ToggleReady => {
                    if let Some(link) = &self.room {
                        link.room.ready(self.id);
                    }
                }
                Effect::RoomRules(rules) => {
                    if let Some(link) = &self.room {
                        link.room.set_rules(self.id, rules);
                    }
                }
                Effect::AddBot => {
                    if let Some(link) = &self.room {
                        link.room.bot(self.id, true);
                    }
                }
                Effect::RemoveBot => {
                    if let Some(link) = &self.room {
                        link.room.bot(self.id, false);
                    }
                }
                Effect::PlayBots => self.shared.matchmaker.do_send(BotMatch(self.id)),
                Effect::Watch(name) => self.watch(name, ctx),
                Effect::LeaveRoom => self.leave_room(ctx),
                Effect::SetNickname(name) => self.rename(&name, ctx),
                Effect::SignUp { username, password } => self.sign_up(username, password, ctx),
                Effect::SignIn {
                    username,
                    password,
                    remember,
                } => self.sign_in(username, password, remember, ctx),
                Effect::SignOut => self.sign_out(ctx),
                Effect::ChangePassword(password) => self.change_password(password, ctx),
                Effect::ForgetKey(key) => self.refresh_keys(Some(key), ctx),
                Effect::LoadAccount => self.refresh_keys(None, ctx),
                Effect::LoadFriends => self.refresh_friends(ctx),
                Effect::LoadLeaderboard => self.load_leaderboard(ctx),
                Effect::AddFriend(name) => self.add_friend(name, ctx),
                Effect::AnswerFriend { name, accept } => self.answer_friend(name, accept, ctx),
                Effect::RemoveFriend(name) => self.remove_friend(name, ctx),
                Effect::Invite(name) => self.invite(name, ctx),
                Effect::AcceptInvite => self.accept_invite(ctx),
                Effect::DismissInvite => self.dismiss_invite(ctx),
            }
        }
    }

    /// The guest name others see, such as `~alice`.
    fn guest_name(&self) -> String {
        self.player
            .as_ref()
            .map_or_else(|| "~player".to_string(), Player::display_name)
    }

    fn entrant(&self) -> Entrant {
        Entrant {
            id: self.id,
            name: match &self.account {
                Some(account) => account.username.clone(),
                None => self.guest_name(),
            },
        }
    }

    fn enqueue(&mut self, ctx: &mut Context<Self>) {
        if self.room.is_some() {
            return;
        }
        self.queued = true;
        self.shared.matchmaker.do_send(Enqueue {
            who: self.entrant(),
            account: self.account_id(),
            rating: self.rating(),
            region: self
                .shared
                .cluster
                .as_ref()
                .map_or_else(|| LOCAL_REGION.to_string(), |c| c.region.clone()),
            presence: Arc::downgrade(&self.presence),
            reply: ctx.address().recipient(),
            on_rated: ctx.address().recipient(),
        });
    }

    /// Makes a private room with `rules` and takes a seat in it.
    fn room_maker(
        &self,
        rules: Rules,
    ) -> impl std::future::Future<Output = Result<JoinedRoom, JoinFail>> + use<> {
        let lobby = self.shared.lobby.clone();
        let seed = self.shared.cfg.room_seed;
        let who = self.entrant();
        let presence = Arc::downgrade(&self.presence);
        async move {
            let room = match lobby.send(CreateRoom { rules, seed }).await {
                Ok(Ok((_, room))) => room,
                Ok(Err(CreateError::BoardTooSmall)) => return Err(JoinFail::BoardTooSmall),
                Ok(Err(CreateError::Unavailable)) => return Err(JoinFail::Unavailable),
                Err(_) => return Err(JoinFail::Closed),
            };
            seat(room, who, presence).await
        }
    }

    /// Makes a private room with `rules`, or finds the one with `code`, and
    /// takes a seat in it.
    fn join_private(
        &mut self,
        rules: Option<Rules>,
        code: Option<String>,
        ctx: &mut Context<Self>,
    ) {
        if code.is_some() && self.wrong_codes >= MAX_WRONG_CODES {
            return self.join_failed(JoinFail::TooManyCodes, ctx);
        }
        let join: std::pin::Pin<Box<dyn std::future::Future<Output = _>>> = match (rules, code) {
            (Some(rules), _) => Box::pin(self.room_maker(rules)),
            (None, Some(code)) => {
                let lobby = self.shared.lobby.clone();
                let guesses = self.shared.guesses.clone();
                let ip = self.peer.ip();
                let who = self.entrant();
                let presence = Arc::downgrade(&self.presence);
                Box::pin(async move {
                    // Wrong codes count per address too, or reconnecting
                    // would give a fresh five each time.
                    match guesses.may_try_code(ip, now()).await {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => return Err(JoinFail::CodesFromAddress),
                        Err(_) => return Err(JoinFail::Unavailable),
                    }
                    match lobby.send(FindRoom(code)).await {
                        Ok(Some(room)) => seat(room, who, presence).await,
                        Ok(None) => {
                            let _ = guesses.wrong_code(ip, now()).await;
                            Err(JoinFail::NoSuchCode)
                        }
                        Err(_) => Err(JoinFail::Closed),
                    }
                })
            }
            (None, None) => return,
        };
        ctx.spawn(join.into_actor(self).map(|joined, act, ctx| match joined {
            Ok((room, view)) => act.entered(room, view, ctx),
            Err(fail) => act.join_failed(fail, ctx),
        }));
    }

    fn join_failed(&mut self, fail: JoinFail, ctx: &mut Context<Self>) {
        if matches!(fail, JoinFail::NoSuchCode) {
            self.wrong_codes += 1;
            debug!(peer = %self.peer, tries = self.wrong_codes, "wrong room code");
        }
        if let Some(app) = self.app.as_mut() {
            app.join_failed(fail.message().to_string());
        }
        self.render(ctx);
    }

    /// The player now has a seat in `room`. If they've stopped wanting one
    /// meanwhile, they give it straight back.
    fn entered(&mut self, room: RoomHandle, view: ViewRx, ctx: &mut Context<Self>) {
        let wanted = self.app.as_ref().is_some_and(|a| {
            matches!(a.screen, Screen::Finding | Screen::Joining { .. }) && !a.in_room()
        });
        if self.closing || !wanted || self.room.is_some() {
            room.leave(self.id);
            return;
        }
        let current = view.borrow().clone();
        if let Some(app) = self.app.as_mut() {
            app.enter_room(self.id, current);
        }
        self.link(room, view, ctx);
    }

    /// Follows `room`'s views from now on.
    fn link(&mut self, room: RoomHandle, view: ViewRx, ctx: &mut Context<Self>) {
        self.stays += 1;
        let stay = self.stays;
        let watch = self.watch_room(view, stay, ctx);
        if let Some(ping) = self.ping {
            room.ping(self.id, ping);
        }
        self.room = Some(RoomLink { room, watch, stay });
        self.report_status();
        self.render(ctx);
    }

    /// The player now watches `friend` in `room`, without a seat. If they've
    /// stopped wanting to meanwhile, they let it go.
    fn watching(
        &mut self,
        room: RoomHandle,
        view: ViewRx,
        friend: String,
        ctx: &mut Context<Self>,
    ) {
        let wanted = self
            .app
            .as_ref()
            .is_some_and(|a| a.screen == Screen::JoiningToWatch && !a.in_room());
        if self.closing || !wanted || self.room.is_some() {
            room.leave(self.id);
            return;
        }
        let current = view.borrow().clone();
        if let Some(app) = self.app.as_mut() {
            app.watch_room(self.id, friend, current);
        }
        self.link(room, view, ctx);
    }

    /// Waits for the room's next view; each one re-arms the wait.
    fn watch_room(&mut self, mut view: ViewRx, stay: u64, ctx: &mut Context<Self>) -> SpawnHandle {
        let next = async move {
            let open = view.changed().await.is_ok();
            (open, view)
        };
        ctx.spawn(
            next.into_actor(self)
                .map(move |(open, mut view), act, ctx| {
                    if act.room.as_ref().is_none_or(|r| r.stay != stay) {
                        return;
                    }
                    let latest = view.borrow_and_update().clone();
                    let Some(app) = act.app.as_mut() else { return };
                    if open {
                        app.update_room(latest);
                    } else {
                        // The room is gone. Its last view may say why.
                        let reason = match &latest.phase {
                            PhaseView::Closed { reason } => Some(reason.clone()),
                            _ => None,
                        };
                        app.room_closed(reason);
                    }
                    if open && app.in_room() {
                        let watch = act.watch_room(view, stay, ctx);
                        if let Some(room) = act.room.as_mut() {
                            room.watch = watch;
                        }
                    } else {
                        act.room = None;
                    }
                    act.report_status();
                    act.render(ctx);
                    act.leave_if_free(ctx);
                }),
        )
    }

    fn leave_room(&mut self, ctx: &mut Context<Self>) {
        if let Some(link) = self.room.take() {
            ctx.cancel_future(link.watch);
            link.room.leave(self.id);
        }
    }
}

/// What's saved for whoever is connecting.
enum Loaded {
    /// The key is remembered by an account.
    Account(AccountData),
    /// A guest: the saved nickname, if the key has one, settings and records.
    Guest(Option<String>, GameConfig, Records),
}

/// Takes a seat in `room`.
async fn seat(
    room: RoomHandle,
    who: Entrant,
    presence: std::sync::Weak<()>,
) -> Result<JoinedRoom, JoinFail> {
    match room.join(who, presence).await {
        Ok(view) => Ok((room, view)),
        Err(JoinError::Full) => Err(JoinFail::Full),
        Err(JoinError::Closed) => Err(JoinFail::Closed),
    }
}

impl SessionActor {
    fn rename(&mut self, name: &str, ctx: &mut Context<Self>) {
        if self.account.is_some() {
            return;
        }
        let Some(player) = self.player.as_mut() else {
            return;
        };
        let nick = nickname(name, &player.tag);
        if nick != player.nick {
            player.nick = nick;
            self.unsaved.nick = true;
        }
        let display = player.display_name();
        if let Some(app) = self.app.as_mut() {
            app.set_name(display);
        }
        self.save_soon(ctx);
        self.render(ctx);
    }

    /// Saves the settings and nickname soon: once changes stop for a
    /// moment, so a flood of them is one write.
    fn save_soon(&mut self, ctx: &mut Context<Self>) {
        if self.save_timer.is_none() {
            self.save_timer = Some(ctx.run_later(SAVE_AFTER, |act, _| {
                act.save_timer = None;
                act.save_now();
            }));
        }
    }

    /// Writes whatever changed, without waiting: a session that ends
    /// meanwhile still saves it.
    fn save_now(&mut self) {
        let unsaved = std::mem::take(&mut self.unsaved);
        let store = self.shared.store.clone();
        let (account, fingerprint) = (self.account_id(), self.fingerprint());
        if unsaved.config
            && let Some(cfg) = self.app.as_ref().map(|a| a.cfg.clone())
        {
            let (store, fingerprint) = (store.clone(), fingerprint.clone());
            actix::spawn(async move {
                let saved = match (account, fingerprint) {
                    (Some(id), _) => store.save_account_config(id, &cfg).await,
                    (None, Some(fp)) => store.save_config(&fp, &cfg).await,
                    (None, None) => Ok(()),
                };
                if let Err(e) = saved {
                    warn!(error = %e, "couldn't save settings");
                }
            });
        }
        if unsaved.nick
            && let (Some(fp), Some(player)) = (fingerprint, self.player.as_ref())
        {
            let nick = player.nick.clone();
            actix::spawn(async move {
                if let Err(e) = store.set_nick(&fp, &nick).await {
                    warn!(error = %e, "couldn't save a nickname");
                }
            });
        }
    }

    /// Refuses an action: the player is going faster than anyone would.
    fn too_fast(&mut self, ctx: &mut Context<Self>) {
        if let Some(app) = self.app.as_mut() {
            let why = TOO_FAST.to_string();
            match app.screen {
                Screen::Joining { .. } => app.join_failed(why),
                Screen::JoiningToWatch => app.watch_failed(why),
                Screen::Form(_) => app.form_failed(why),
                _ => app.notify(why),
            }
        }
        self.render(ctx);
    }

    /// Draws the current screen soon: at once, unless a frame went out
    /// under [`FRAME`] ago, so a flood of keys doesn't make a frame each.
    fn render(&mut self, ctx: &mut Context<Self>) {
        if self.redraw.is_some() {
            return;
        }
        let wait = self
            .drawn_at
            .map_or(Duration::ZERO, |at| FRAME.saturating_sub(at.elapsed()));
        if wait.is_zero() {
            return self.draw(ctx);
        }
        self.redraw = Some(ctx.run_later(wait, |act, ctx| {
            act.redraw = None;
            act.draw(ctx);
        }));
    }

    /// Draws the current screen and queues it for the client.
    fn draw(&mut self, ctx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let (Some(app), Some(terminal), Some(out)) = (&self.app, &mut self.terminal, &self.out)
        else {
            return;
        };
        if out.capacity() == 0 {
            // The client is behind. Its screen still matches what ratatui
            // last sent, so just draw again once it catches up.
            let since = *self.stalled_since.get_or_insert_with(|| {
                debug!(peer = %self.peer, "client is behind; holding frames");
                Instant::now()
            });
            if since.elapsed() > self.shared.cfg.stall_timeout() {
                info!(peer = %self.peer, "client stopped reading; disconnecting");
                self.save_unfinished_game(ctx);
                return self.close(1, None, ctx);
            }
            if self.retry.is_none() {
                self.retry = Some(ctx.run_later(RETRY_DRAW, |act, ctx| {
                    act.retry = None;
                    act.render(ctx);
                }));
            }
            return;
        }
        self.stalled_since = None;
        self.drawn_at = Some(Instant::now());
        let drawn = catch_unwind(AssertUnwindSafe(|| {
            terminal.draw(|frame| ui::draw(frame, app)).map(|_| ())
        }));
        let bytes = terminal.backend_mut().take_output();
        if !bytes.is_empty() {
            let _ = out.try_send(Out::Data(bytes.into()));
        }
        match drawn {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(peer = %self.peer, error = %e, "drawing failed");
                self.close(1, None, ctx);
            }
            Err(_) => {
                warn!(peer = %self.peer, "drawing panicked");
                self.close(1, None, ctx);
            }
        }
    }

    /// While a game runs, wakes up when its next step is due.
    fn schedule_wake(&mut self, ctx: &mut Context<Self>) {
        if let Some(wake) = self.wake.take() {
            ctx.cancel_future(wake);
        }
        let Some(app) = &self.app else { return };
        if self.closing {
            return;
        }
        let delay = match (app.screen, &app.game) {
            // The queue's clock and size on screen.
            (Screen::Finding, _) => QUEUE_WAKE,
            // Frozen until the first key; nothing to wake for.
            (Screen::Playing, Some(game)) if !game.started => return,
            (Screen::Playing, Some(game)) => game
                .until_next_step()
                .unwrap_or_default()
                .clamp(Duration::from_millis(1), MAX_WAKE),
            _ => return,
        };
        self.wake = Some(ctx.run_later(delay, |act, ctx| {
            act.wake = None;
            act.on_wake(ctx);
        }));
    }

    fn on_wake(&mut self, ctx: &mut Context<Self>) {
        let Some(app) = self.app.as_mut() else { return };
        let now = Instant::now();
        let effects = app.tick(now - self.last_tick);
        app.set_queue_len(*self.shared.queue_len.borrow());
        self.last_tick = now;
        self.apply(effects, ctx);
        self.render(ctx);
        self.schedule_wake(ctx);
    }

    fn check_idle(&mut self, ctx: &mut Context<Self>) {
        let idle = self.shared.cfg.idle_timeout();
        // Waiting for a match or watching a round isn't idling.
        let playing = self.app.as_ref().is_some_and(|a| {
            matches!(a.screen, Screen::Playing | Screen::Finding)
                || a.online
                    .as_ref()
                    .and_then(|o| o.room.as_ref())
                    .is_some_and(|r| !matches!(r.view.phase, PhaseView::Waiting { .. }))
        });
        if self.app.is_some() && !playing && self.last_input.elapsed() > idle {
            info!(peer = %self.peer, "idle; logging out");
            self.close(0, Some("Logged out after being idle.\r\n"), ctx);
        }
    }

    /// Leaves the game screen, says `message` if any, and ends the
    /// connection with `exit_status`.
    fn close(&mut self, exit_status: u32, message: Option<&'static str>, ctx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        if self.app.is_some() {
            self.send(Out::Data(Bytes::from_static(LEAVE_SCREEN)));
        }
        if let Some(message) = message {
            self.send(Out::Data(Bytes::from_static(message.as_bytes())));
        }
        if !self.send(Out::Close { exit_status })
            && let Some(hangup) = &self.hangup
        {
            // No channel yet, or the client is stuck: just hang up.
            hangup.now();
        }
        ctx.stop();
    }

    /// The game ends without the player quitting: keep the record it reached.
    /// The server is stopping: keeps the game's record, and logs out.
    fn goodbye(&mut self, ctx: &mut Context<Self>) {
        self.save_unfinished_game(ctx);
        self.close(0, Some(SHUTTING_DOWN), ctx);
    }

    /// While the server drains, players go as soon as they aren't in a
    /// round, after a moment to see its results.
    fn leave_if_free(&mut self, ctx: &mut Context<Self>) {
        if !self.draining || self.closing || self.leaving.is_some() {
            return;
        }
        let phase = self
            .app
            .as_ref()
            .and_then(|a| a.online.as_ref())
            .and_then(|o| o.room.as_ref())
            .map(|r| &r.view.phase);
        match phase {
            Some(PhaseView::Countdown { .. } | PhaseView::Running { .. }) => {}
            Some(PhaseView::Results { .. }) => {
                self.leaving = Some(ctx.run_later(RESULTS_GRACE, |act, ctx| act.goodbye(ctx)));
            }
            _ => self.goodbye(ctx),
        }
    }

    fn save_unfinished_game(&mut self, ctx: &mut Context<Self>) {
        if let Some(app) = self.app.as_mut() {
            let effects: Vec<Effect> = app
                .handle(Key::CtrlC)
                .into_iter()
                .filter(|e| matches!(e, Effect::RecordSet { .. }))
                .collect();
            self.apply(effects, ctx);
        }
    }
}

/// Sends queued output to the client, in order, until told to close; then
/// makes sure the connection ends.
async fn write_out(
    handle: Handle,
    channel: ChannelId,
    hangup: Option<Hangup>,
    mut rx: mpsc::Receiver<Out>,
) {
    while let Some(out) = rx.recv().await {
        match out {
            Out::Data(data) => {
                if handle.data(channel, data).await.is_err() {
                    break;
                }
            }
            Out::Close { exit_status } => {
                let goodbye = async {
                    let _ = handle.exit_status_request(channel, exit_status).await;
                    let _ = handle.eof(channel).await;
                    let _ = handle.close(channel).await;
                };
                let _ = tokio::time::timeout(HANG_UP_AFTER, goodbye).await;
                // Clients hang up once their only channel closes; hang up
                // ourselves only on one that doesn't.
                tokio::time::sleep(HANG_UP_AFTER).await;
                break;
            }
        }
    }
    if let Some(hangup) = hangup {
        hangup.now();
    }
}

impl Handler<Attach> for SessionActor {
    type Result = ();

    fn handle(&mut self, Attach(hangup): Attach, ctx: &mut Self::Context) {
        self.hangup = Some(hangup);
        ctx.run_later(self.shared.cfg.login_timeout(), |act, ctx| {
            if act.app.is_none() {
                debug!(peer = %act.peer, "didn't open the game in time");
                act.close(1, None, ctx);
            }
        });
    }
}

impl Handler<Authenticated> for SessionActor {
    type Result = ();

    fn handle(&mut self, Authenticated(player): Authenticated, _: &mut Self::Context) {
        debug!(peer = %self.peer, nick = %player.nick, "signed in");
        self.player = Some(player);
    }
}

impl Handler<ChannelOpened> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: ChannelOpened, _: &mut Self::Context) {
        let (tx, rx) = mpsc::channel(OUT_QUEUE);
        actix::spawn(write_out(msg.handle, msg.channel, self.hangup.clone(), rx));
        self.out = Some(tx);
    }
}

impl Handler<PtyRequested> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: PtyRequested, _: &mut Self::Context) {
        self.size = Self::clamp_size(msg.cols, msg.rows);
    }
}

impl Handler<ShellRequested> for SessionActor {
    type Result = ();

    fn handle(&mut self, _: ShellRequested, ctx: &mut Self::Context) {
        if !self.shell_requested && !self.closing {
            self.shell_requested = true;
            self.start(ctx);
        }
    }
}

impl Handler<Refuse> for SessionActor {
    type Result = ();

    fn handle(&mut self, Refuse(why): Refuse, ctx: &mut Self::Context) {
        debug!(peer = %self.peer, "refused: {}", why.trim_end());
        self.close(1, Some(why), ctx);
    }
}

impl Handler<Input> for SessionActor {
    type Result = ();

    fn handle(&mut self, Input(bytes): Input, ctx: &mut Self::Context) {
        if self.closing || self.app.is_none() {
            return;
        }
        if let Some(timer) = self.esc_timer.take() {
            ctx.cancel_future(timer);
        }
        let mut keys = Vec::new();
        self.parser.feed(&bytes, &mut keys);
        let allowed = self.keys.take(keys.len(), Instant::now());
        keys.truncate(allowed);
        let answers = self.parser.take_reports();
        // The terminal answering a status query isn't the player doing
        // anything: it mustn't keep an idle player connected.
        if answers == 0 || !keys.is_empty() || self.parser.pending_esc() {
            self.last_input = Instant::now();
        }
        if answers > 0 {
            self.on_pong();
        }
        if self.parser.pending_esc() {
            self.esc_timer = Some(ctx.run_later(ESC_WAIT, |act, ctx| {
                act.esc_timer = None;
                let mut keys = Vec::new();
                act.parser.flush(&mut keys);
                act.on_keys(keys, ctx);
            }));
        }
        if !keys.is_empty() {
            self.on_keys(keys, ctx);
        }
    }
}

impl Handler<Resized> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: Resized, ctx: &mut Self::Context) {
        self.size = Self::clamp_size(msg.cols, msg.rows);
        let area = Rect::new(0, 0, self.size.width, self.size.height);
        if let Some(terminal) = self.terminal.as_mut() {
            terminal.backend_mut().set_size(self.size);
            if let Err(e) = terminal.resize(area) {
                warn!(peer = %self.peer, error = %e, "couldn't resize");
            }
        }
        if let Some(app) = self.app.as_mut() {
            app.resize(self.size);
        }
        self.render(ctx);
        self.schedule_wake(ctx);
    }
}

impl Handler<Matched> for SessionActor {
    type Result = ();

    fn handle(&mut self, msg: Matched, ctx: &mut Self::Context) {
        self.queued = false;
        self.entered(msg.room, msg.view, ctx);
    }
}

impl Handler<Disconnected> for SessionActor {
    type Result = ();

    fn handle(&mut self, _: Disconnected, ctx: &mut Self::Context) {
        if !self.closing {
            self.save_unfinished_game(ctx);
            self.closing = true;
            ctx.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_of_loads_is_one_load_and_one_more() {
        let mut load = Load::default();
        assert!(load.start());
        for _ in 0..1000 {
            assert!(!load.start(), "one is running");
        }
        assert!(load.done(), "once more, for what was asked meanwhile");
        assert!(load.start());
        assert!(!load.done(), "nothing asked meanwhile");
    }

    #[test]
    fn database_actions_are_costly_and_moves_are_not() {
        assert!(costly(&Effect::AddFriend("ann".into())));
        assert!(costly(&Effect::JoinRoom("ABCDEF".into())));
        assert!(!costly(&Effect::Steer(snake_core::geom::Direction::Up)));
        assert!(!costly(&Effect::LoadLeaderboard), "coalesced instead");
    }

    #[test]
    fn pty_sizes_are_clamped() {
        assert_eq!(SessionActor::clamp_size(100, 30), Size::new(100, 30));
        assert_eq!(SessionActor::clamp_size(65535, 65535), Size::new(240, 80));
        assert_eq!(SessionActor::clamp_size(0, 0), Size::new(80, 24));
    }
}
