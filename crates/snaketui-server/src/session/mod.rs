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
use snake_tui::{App, Effect, Key, Screen, ui};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use self::account::{JoinedRoom, PendingInvite};
use crate::Shared;
use crate::backend::SshBackend;
use crate::identity::{Player, nickname};
use crate::input::KeyParser;
use crate::lobby::{CreateError, CreateRoom, FindRoom};
use crate::matchmaker::{Dequeue, Enqueue, Matched};
use crate::presence::Status;
use crate::room::{Join, Leave, Ready, RoomActor, Steer, ViewRx};
use crate::store::{Account, AccountData};

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
/// After the game closes the channel, how long a client gets to hang up.
const HANG_UP_AFTER: Duration = Duration::from_secs(2);
/// Keys and window changes waiting in the mailbox; more are dropped.
const MAILBOX: usize = 64;
/// Wrong room codes a connection may try. After that it can't join by code.
const MAX_WRONG_CODES: u8 = 5;
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

/// The room a session's player is in.
struct RoomLink {
    addr: Addr<RoomActor>,
    /// The future waiting for the room's next view.
    watch: SpawnHandle,
    /// Tells apart this stay from an earlier one in the same room.
    stay: u64,
}

/// Why a private room couldn't be made or joined.
enum JoinFail {
    NoSuchCode,
    TooManyCodes,
    BoardTooSmall,
    Full,
    Closed,
}

impl JoinFail {
    fn message(&self) -> &'static str {
        match self {
            JoinFail::NoSuchCode => "No room has that code.",
            JoinFail::TooManyCodes => "Too many wrong codes. Reconnect to try again.",
            JoinFail::BoardTooSmall => {
                "Your board is too small for two snakes. Make it bigger in Single player."
            }
            JoinFail::Full => "That room is full.",
            JoinFail::Closed => "That room has closed.",
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
    /// What friends were last told this player is doing.
    status: Option<Status>,
    hangup: Option<Hangup>,
    out: Option<mpsc::Sender<Out>>,
    player: Option<Player>,
    size: Size,
    /// Both exist from when the game screen opens.
    terminal: Option<Terminal<SshBackend>>,
    app: Option<App>,
    parser: KeyParser,
    last_tick: Instant,
    last_input: Instant,
    wake: Option<SpawnHandle>,
    esc_timer: Option<SpawnHandle>,
    retry: Option<SpawnHandle>,
    stalled_since: Option<Instant>,
    shell_requested: bool,
    closing: bool,
}

impl Actor for SessionActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(MAILBOX);
        ctx.run_interval(IDLE_CHECK, |act, ctx| act.check_idle(ctx));
        let mut shutdown = self.shared.shutdown.subscribe();
        ctx.spawn(
            async move {
                let _ = shutdown.wait_for(|stopping| *stopping).await;
            }
            .into_actor(self)
            .map(|_, act, ctx| {
                act.save_unfinished_game(ctx);
                act.close(0, Some(SHUTTING_DOWN), ctx)
            }),
        );
    }

    fn stopped(&mut self, _: &mut Self::Context) {
        debug!(peer = %self.peer, "session ended");
        self.sign_off();
        if let Some(room) = self.room.take() {
            room.addr.do_send(Leave(self.id));
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
            status: None,
            hangup: None,
            out: None,
            player: None,
            size: Size::new(80, 24),
            terminal: None,
            app: None,
            parser: KeyParser::default(),
            last_tick: now,
            last_input: now,
            wake: None,
            esc_timer: None,
            retry: None,
            stalled_since: None,
            shell_requested: false,
            closing: false,
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
        app.resize(self.size);
        self.app = Some(app);
        self.terminal = Some(terminal);
        self.last_input = Instant::now();
        self.send(Out::Data(Bytes::from_static(ENTER_SCREEN)));
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
    }

    fn apply(&mut self, effects: Vec<Effect>, ctx: &mut Context<Self>) {
        let fingerprint = self.fingerprint();
        let account = self.account_id();
        let store = self.shared.store.clone();
        for effect in effects {
            match effect {
                Effect::SaveConfig => {
                    let Some(cfg) = self.app.as_ref().map(|a| a.cfg.clone()) else {
                        continue;
                    };
                    let (store, fp) = (store.clone(), fingerprint.clone());
                    actix::spawn(async move {
                        let saved = match (account, fp) {
                            (Some(id), _) => store.save_account_config(id, &cfg).await,
                            (None, Some(fp)) => store.save_config(&fp, &cfg).await,
                            (None, None) => Ok(()),
                        };
                        if let Err(e) = saved {
                            warn!(error = %e, "couldn't save settings");
                        }
                    });
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
                    if let Some(room) = &self.room {
                        let _ = room.addr.try_send(Steer { id: self.id, dir });
                    }
                }
                Effect::ToggleReady => {
                    if let Some(room) = &self.room {
                        let _ = room.addr.try_send(Ready(self.id));
                    }
                }
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
            region: LOCAL_REGION.to_string(),
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
                let who = self.entrant();
                let presence = Arc::downgrade(&self.presence);
                Box::pin(async move {
                    match lobby.send(FindRoom(code)).await {
                        Ok(Some(room)) => seat(room, who, presence).await,
                        Ok(None) => Err(JoinFail::NoSuchCode),
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
    fn entered(&mut self, room: Addr<RoomActor>, view: ViewRx, ctx: &mut Context<Self>) {
        let wanted = self.app.as_ref().is_some_and(|a| {
            matches!(a.screen, Screen::Finding | Screen::Joining { .. }) && !a.in_room()
        });
        if self.closing || !wanted || self.room.is_some() {
            room.do_send(Leave(self.id));
            return;
        }
        let current = view.borrow().clone();
        if let Some(app) = self.app.as_mut() {
            app.enter_room(self.id, current);
        }
        self.stays += 1;
        let stay = self.stays;
        let watch = self.watch_room(view, stay, ctx);
        self.room = Some(RoomLink {
            addr: room,
            watch,
            stay,
        });
        self.report_status();
        self.render(ctx);
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
                }),
        )
    }

    fn leave_room(&mut self, ctx: &mut Context<Self>) {
        if let Some(room) = self.room.take() {
            ctx.cancel_future(room.watch);
            room.addr.do_send(Leave(self.id));
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
    room: Addr<RoomActor>,
    who: Entrant,
    presence: std::sync::Weak<()>,
) -> Result<JoinedRoom, JoinFail> {
    match room.send(Join { who, presence }).await {
        Ok(Ok(view)) => Ok((room, view)),
        Ok(Err(JoinError::Full)) => Err(JoinFail::Full),
        Ok(Err(JoinError::Closed)) | Err(_) => Err(JoinFail::Closed),
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
        player.nick = nickname(name, &player.tag);
        let display = player.display_name();
        if let Some(fp) = player.fingerprint().map(str::to_string) {
            let (store, nick) = (self.shared.store.clone(), player.nick.clone());
            actix::spawn(async move {
                if let Err(e) = store.set_nick(&fp, &nick).await {
                    warn!(error = %e, "couldn't save a nickname");
                }
            });
        }
        if let Some(app) = self.app.as_mut() {
            app.set_name(display);
        }
        self.render(ctx);
    }

    /// Draws the current screen and queues it for the client.
    fn render(&mut self, ctx: &mut Context<Self>) {
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
        self.last_input = Instant::now();
        if let Some(timer) = self.esc_timer.take() {
            ctx.cancel_future(timer);
        }
        let mut keys = Vec::new();
        self.parser.feed(&bytes, &mut keys);
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
    fn pty_sizes_are_clamped() {
        assert_eq!(SessionActor::clamp_size(100, 30), Size::new(100, 30));
        assert_eq!(SessionActor::clamp_size(65535, 65535), Size::new(240, 80));
        assert_eq!(SessionActor::clamp_size(0, 0), Size::new(80, 24));
    }
}
