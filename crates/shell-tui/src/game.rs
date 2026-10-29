//! What a game adds to the app: its own screens, how its rooms are made
//! and matched, and how its rounds look. Each game implements [`GameUi`];
//! the app does the rest the same way for every game.

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use game_api::{AnyView, GameId, PhaseView, PlayerId, Records};
use ratatui::Frame;
use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::layout::{Rect, Size};
use ratatui::style::Color;
use ratatui::text::Span;

use crate::key::Key;
use crate::online::{Category, InRoom, Profile};

/// Work a game leaves to whoever runs it, in the order given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GameFx {
    /// A move in the round being played, as the game's rooms read it.
    Play(Vec<u8>),
    /// The host's settings for the room's next rounds, as its rooms read
    /// them.
    Settings(Vec<u8>),
    /// Keep the player's preferences, as the game writes them.
    SavePrefs(String),
    /// A new best was just set. By the time this is handled the game may
    /// be gone, so it says which record changed.
    Record { key: String, best: u32 },
}

/// Where a key on a game's own screens leaves the player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Next {
    /// Where they were.
    #[default]
    Stay,
    /// On the game's own screens.
    Game,
    /// Back to the home menu; a game played on its own ends.
    Home,
    /// Back to the room they're in.
    Room,
    /// Into quick play, waiting in this pool of the game's.
    Queue(&'static str),
}

/// What a game made of a key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub fx: Vec<GameFx>,
    pub next: Next,
    /// Something to tell the player.
    pub notice: Option<String>,
}

impl Reply {
    pub fn fx(fx: GameFx) -> Self {
        Self {
            fx: vec![fx],
            ..Self::default()
        }
    }

    pub fn to(next: Next) -> Self {
        Self {
            next,
            ..Self::default()
        }
    }

    pub fn notice(notice: String) -> Self {
        Self {
            notice: Some(notice),
            ..Self::default()
        }
    }
}

/// What a game's screens see of the app around them.
#[derive(Clone, Copy, Debug)]
pub struct Ctx<'a> {
    pub size: Size,
    /// Played over the network, inside the app, rather than on its own.
    pub online: bool,
    /// The room the player is in.
    pub room: Option<&'a InRoom>,
    /// The account they're signed in to, if any.
    pub account: Option<&'a Profile>,
    /// Where to put pictures, if the player's terminal shows them.
    pub pictures: Option<&'a Pictures>,
    /// Something to tell the player, until their next key.
    pub notice: Option<&'a str>,
}

impl Ctx<'_> {
    /// A game played on its own, on a terminal of `size`.
    pub fn alone(size: Size) -> Self {
        Self {
            size,
            online: false,
            room: None,
            account: None,
            pictures: None,
            notice: None,
        }
    }
}

/// What's kept for a player in one game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    /// Their preferences, as the game writes them; empty for none yet.
    pub prefs: String,
    pub records: Records,
}

/// A picture, for terminals that show them: PNG data, and an id no other
/// picture has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub id: u64,
    pub png: Arc<[u8]>,
}

/// A picture over `area`, and the cells under it as they were drawn: what
/// shows wherever the picture can't.
#[derive(Clone, Debug)]
pub struct Placement {
    pub area: Rect,
    pub picture: Picture,
    under: Vec<Cell>,
}

/// The pictures a frame shows, in a terminal that shows them.
#[derive(Debug)]
pub struct Pictures {
    /// A cell's size in pixels, width then height.
    pub cell: (u16, u16),
    placed: Mutex<Vec<Placement>>,
}

impl Pictures {
    pub fn new(cell: (u16, u16)) -> Self {
        Self {
            cell,
            placed: Mutex::default(),
        }
    }

    /// Shows `picture` over `area`, where `buf` holds what to show instead
    /// if the picture can't.
    pub fn place(&self, buf: &Buffer, area: Rect, picture: Picture) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }
        let under = area.positions().map(|at| buf[at].clone()).collect();
        self.placed.lock().unwrap().push(Placement {
            area,
            picture,
            under,
        });
    }

    /// A new frame starts, with none yet.
    pub(crate) fn clear(&self) {
        self.placed.lock().unwrap().clear();
    }

    /// The frame is drawn: keeps the pictures nothing was drawn over since
    /// they were put there, and keeps text from being drawn under those.
    /// A menu over the board hides the pieces under it this way.
    pub(crate) fn settle(&self, buf: &mut Buffer) {
        self.placed.lock().unwrap().retain(|p| {
            let untouched = p
                .area
                .positions()
                .zip(&p.under)
                .all(|(at, cell)| buf.cell(at) == Some(cell));
            if untouched {
                for at in p.area.positions() {
                    if let Some(cell) = buf.cell_mut(at) {
                        cell.set_diff_option(CellDiffOption::Skip);
                    }
                }
            }
            untouched
        });
    }

    /// The pictures the frame shows, for whoever draws them.
    pub fn take(&self) -> Vec<Placement> {
        std::mem::take(&mut *self.placed.lock().unwrap())
    }
}

/// What choosing quick play does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickStart {
    /// Waits in one of the game's pools.
    Queue(QuickPlay),
    /// Opens the game's own screens, where the player picks a pool and
    /// answers with [`Next::Queue`].
    Choose,
}

/// How a player gets into quick play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickPlay {
    /// The pool they wait in, one of the game's.
    pub pool: &'static str,
    /// Why their terminal is too small for its matches, if it is. Choosing
    /// quick play again queues anyway.
    pub too_small: Option<String>,
}

/// One game as the app shows it. Rooms' views come as [`AnyView`]s of the
/// game's rooms, which the game reads with [`AnyView::of`].
pub trait GameUi: Any + Send + Sync {
    fn id(&self) -> GameId;
    /// Its name, as the game picker lists it.
    fn title(&self) -> &'static str;
    /// Its name drawn big, at the top of the home menu, and in what color.
    fn logo(&self) -> (&'static [&'static str], Color);

    // The home menu.

    /// Entries at the top of the home menu that open the game's own
    /// screens, such as Snake's single player.
    fn home_items(&self) -> &'static [&'static str] {
        &[]
    }

    /// Home entry `item` was chosen: its screens show next.
    fn open(&mut self, item: usize) {
        let _ = item;
    }

    /// A note beside "Create a private room", such as what it's made with.
    fn room_note(&self) -> Option<String> {
        None
    }

    /// A private room's settings, as the game's rooms read them, for one
    /// this player makes.
    fn room_settings(&self) -> Vec<u8>;

    /// How this player, on a terminal of `size`, gets into quick play; or
    /// why they can't yet.
    fn quick_play(&mut self, size: Size) -> Result<QuickStart, String>;

    /// One of its pools as the player knows it, such as chess's "5+3
    /// blitz", while they wait in it.
    fn pool_name(&self, pool: &str) -> Option<String> {
        let _ = pool;
        None
    }

    /// Its ratings, at least one, each with its own leaderboard; the first
    /// is the one the home menu shows.
    fn ratings(&self) -> &'static [Category];

    /// Whether its rooms take bots, which hosts add and quick play offers.
    fn bots(&self) -> bool {
        false
    }

    // The game's own screens: what shows while the app's screen is
    // [`crate::Screen::Game`]. Keys come to a game as typed, Shift and all;
    // one that doesn't care makes them [`Key::lower`].

    fn key(&mut self, key: Key, ctx: &Ctx) -> Reply {
        let _ = (key, ctx);
        Reply::to(Next::Home)
    }

    /// Real time went by: `dt` of it.
    fn tick(&mut self, dt: Duration) -> Vec<GameFx> {
        let _ = dt;
        Vec::new()
    }

    /// How soon [`Self::tick`] has something to do; `None` until a key.
    fn next_wake(&self) -> Option<Duration> {
        None
    }

    fn resize(&mut self, size: Size) {
        let _ = size;
    }

    /// Whether a game is being played on these screens, so invites wait
    /// and time counts toward it.
    fn playing(&self) -> bool {
        false
    }

    fn draw(&self, frame: &mut Frame, ctx: &Ctx) {
        let _ = (frame, ctx);
    }

    /// Whether its own screens take the mouse: while they show, the app
    /// turns the mouse on and passes [`Key::Mouse`] on. Elsewhere the mouse
    /// stays off, and selects text.
    fn wants_mouse(&self) -> bool {
        false
    }

    // Rooms. The app runs their lobby and results; the game adds to them
    // and draws its rounds.

    /// Whether the player has a part in the round being played, so leaving
    /// asks first and invites wait. By default, anyone seated in it.
    fn in_play(&self, room: &InRoom) -> bool {
        matches!(
            room.shell().phase,
            PhaseView::Countdown { .. } | PhaseView::Running { .. }
        ) && room.my_seat().is_some_and(|s| s.slot.is_some())
    }

    /// Whether the room's round, as it is now, takes the mouse, as
    /// [`Self::wants_mouse`] for the game's own screens.
    fn room_wants_mouse(&self, room: &InRoom) -> bool {
        let _ = room;
        false
    }

    /// A key in the room, before the app's own room keys; `None` if it's
    /// not one of the game's.
    fn room_key(&mut self, key: Key, room: &InRoom) -> Option<Reply> {
        let _ = (key, room);
        None
    }

    /// What the host changed going from `old` to the room's settings now,
    /// to tell everyone else.
    fn settings_note(&self, old: &dyn AnyView, room: &InRoom) -> Option<String> {
        let _ = (old, room);
        None
    }

    /// The lobby's line about the room's settings.
    fn lobby_line(&self, room: &InRoom) -> Vec<Span<'static>> {
        let _ = room;
        Vec::new()
    }

    /// The game's keys in the lobby, for its hint line, such as "e rules".
    fn lobby_keys(&self) -> Option<&'static str> {
        None
    }

    /// The host's own keys in the lobby, before the app's for bots.
    fn host_keys(&self) -> Option<&'static str> {
        None
    }

    /// The room from its countdown on, under the results when there are
    /// some.
    fn draw_round(&self, frame: &mut Frame, room: &InRoom, ctx: &Ctx);

    /// A line under the results' headline, such as how the round ended.
    fn result_note(&self, room: &InRoom) -> Option<String> {
        let _ = room;
        None
    }

    /// What `who` scored in the round the results are for, in four
    /// columns at most.
    fn score(&self, room: &InRoom, who: PlayerId) -> String {
        let _ = (room, who);
        String::new()
    }

    // What's kept for the player.

    /// What was kept for the player, from when they last played.
    fn restore(&mut self, saved: Saved) {
        let _ = saved;
    }

    /// What to keep for the player now, such as a guest's when they make
    /// an account.
    fn saved(&self) -> Saved {
        Saved::default()
    }

    /// The app is closing: what to keep of a game in progress.
    fn quit(&mut self) -> Vec<GameFx> {
        Vec::new()
    }
}
