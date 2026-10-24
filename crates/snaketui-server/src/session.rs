//! One player's connection: their screen, their keys and their game.

use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::prelude::*;
use bytes::Bytes;
use ratatui::layout::{Rect, Size};
use ratatui::{Terminal, TerminalOptions, Viewport};
use russh::ChannelId;
use russh::server::Handle;
use snake_core::config::GameConfig;
use snake_core::records::Records;
use snake_tui::{App, Effect, Key, Screen, ui};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::Shared;
use crate::backend::SshBackend;
use crate::identity::Player;
use crate::input::KeyParser;

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
/// How soon to retry drawing for a client that's behind.
const RETRY_DRAW: Duration = Duration::from_millis(50);
const IDLE_CHECK: Duration = Duration::from_secs(15);
/// After the game closes the channel, how long a client gets to hang up.
const HANG_UP_AFTER: Duration = Duration::from_secs(2);
/// Keys and window changes waiting in the mailbox; more are dropped.
const MAILBOX: usize = 64;

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

pub struct SessionActor {
    shared: Arc<Shared>,
    peer: SocketAddr,
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
    }
}

impl SessionActor {
    pub fn new(shared: Arc<Shared>, peer: SocketAddr) -> Self {
        let now = Instant::now();
        Self {
            shared,
            peer,
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

    /// Loads a returning player's settings and records, then opens the game.
    fn start(&mut self, ctx: &mut Context<Self>) {
        let store = self.shared.store.clone();
        let player = self.player.clone();
        let load = async move {
            match player {
                Some(p) if p.fingerprint().is_some() => {
                    let saved = store.visit(p.fingerprint().unwrap(), &p.nick).await?;
                    Ok((saved.cfg, saved.records))
                }
                _ => anyhow::Ok((GameConfig::default(), Records::default())),
            }
        };
        // Keys that arrive meanwhile wait in the mailbox.
        ctx.wait(load.into_actor(self).map(|loaded, act, ctx| {
            let (cfg, records) = loaded.unwrap_or_else(|e| {
                warn!(peer = %act.peer, error = %e, "couldn't load the player; starting fresh");
                (GameConfig::default(), Records::default())
            });
            act.open_screen(cfg, records, ctx);
        }));
    }

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
        let mut app = App::new(cfg, records);
        app.resize(self.size);
        self.app = Some(app);
        self.terminal = Some(terminal);
        self.last_input = Instant::now();
        self.send(Out::Data(Bytes::from_static(ENTER_SCREEN)));
        self.render(ctx);
    }

    fn on_keys(&mut self, keys: Vec<Key>, ctx: &mut Context<Self>) {
        let Some(app) = self.app.as_mut() else { return };
        let was_playing = app.screen == Screen::Playing;
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
        self.render(ctx);
        self.schedule_wake(ctx);
    }

    fn apply(&mut self, effects: Vec<Effect>, ctx: &mut Context<Self>) {
        let fingerprint = self.fingerprint();
        for effect in effects {
            match effect {
                Effect::SaveConfig => {
                    if let (Some(fp), Some(app)) = (fingerprint.clone(), &self.app) {
                        let (store, cfg) = (self.shared.store.clone(), app.cfg.clone());
                        actix::spawn(async move {
                            if let Err(e) = store.save_config(&fp, &cfg).await {
                                warn!(error = %e, "couldn't save settings");
                            }
                        });
                    }
                }
                Effect::RecordSet { key, best } => {
                    if let Some(fp) = fingerprint.clone() {
                        let store = self.shared.store.clone();
                        actix::spawn(async move {
                            if let Err(e) = store.save_record(&fp, &key, best).await {
                                warn!(error = %e, "couldn't save a record");
                            }
                        });
                    }
                }
                Effect::Quit => self.close(0, None, ctx),
            }
        }
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
        if self.closing || app.screen != Screen::Playing {
            return;
        }
        let delay = match &app.game {
            // Frozen until the first key; nothing to wake for.
            Some(game) if !game.started => return,
            Some(game) => game.until_next_step().unwrap_or_default(),
            None => return,
        };
        let delay = delay.clamp(Duration::from_millis(1), MAX_WAKE);
        self.wake = Some(ctx.run_later(delay, |act, ctx| {
            act.wake = None;
            act.on_wake(ctx);
        }));
    }

    fn on_wake(&mut self, ctx: &mut Context<Self>) {
        let Some(app) = self.app.as_mut() else { return };
        let now = Instant::now();
        let effects = app.tick(now - self.last_tick);
        self.last_tick = now;
        self.apply(effects, ctx);
        self.render(ctx);
        self.schedule_wake(ctx);
    }

    fn check_idle(&mut self, ctx: &mut Context<Self>) {
        let idle = self.shared.cfg.idle_timeout();
        let playing = self
            .app
            .as_ref()
            .is_some_and(|a| a.screen == Screen::Playing);
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
