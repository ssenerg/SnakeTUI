//! Chess's part of the app: its settings, a private room's challenge, and
//! the keys that play a game.

use chess_core::control::{PRESETS, TimeControl};
use chess_core::moves::Move;
use chess_core::piece::{self, Piece};
use chess_core::position::Position;
use chess_core::round::{Chess, ChessInput, ChessSettings, Side};
use chess_core::square::Square;
use chess_core::view::GameView;
use game_api::codec::encode;
use game_api::{AnyView, GameId, PhaseView, PlayerId, RoomView};
use ratatui::Frame;
use ratatui::layout::Size;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use shell_tui::online::{Category, InRoom};
use shell_tui::{Ctx, GameFx, GameUi, Key, Next, QuickPlay, Reply, Saved};

use crate::entry::{self, Entered, MAX_TYPED};
use crate::prefs::{self, MoveInput, Prefs};
use crate::ui;

/// Chess's ratings, one per category of time control, by
/// [`chess_core::control::Category::name`].
const RATINGS: [Category; 4] = [
    Category {
        id: "bullet",
        label: "Bullet",
    },
    Category {
        id: "blitz",
        label: "Blitz",
    },
    Category {
        id: "rapid",
        label: "Rapid",
    },
    Category {
        id: "classical",
        label: "Classical",
    },
];

/// Chess's own screens, while the app shows [`shell_tui::Screen::Game`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChessScreen {
    /// The player's preferences, at [`ChessUi::selected`].
    #[default]
    Settings,
    /// A private room's challenge, at row `selected`: everyone reads it,
    /// the host changes it.
    Challenge { selected: usize },
}

/// What the menu a player opens with Esc during their game offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    OfferDraw,
    AcceptDraw,
    DeclineDraw,
    Resign,
    Abort,
    Flip,
    Back,
}

impl MenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::OfferDraw => "Offer a draw",
            MenuItem::AcceptDraw => "Accept the draw",
            MenuItem::DeclineDraw => "Decline the draw",
            MenuItem::Resign => "Resign",
            MenuItem::Abort => "Abort the game",
            MenuItem::Flip => "Flip the board",
            MenuItem::Back => "Back to the game",
        }
    }

    /// What the menu offers `me` in `game`: answering a draw offer or
    /// making one, resigning, or aborting before both sides have moved.
    pub fn offered(game: &GameView, me: piece::Color) -> Vec<MenuItem> {
        let mut items = Vec::new();
        let first_moves = game.first_move_deadline.is_some();
        match game.draw_offer {
            Some(by) if by != me => items.extend([MenuItem::AcceptDraw, MenuItem::DeclineDraw]),
            // Theirs stands, or there's no game to draw yet.
            Some(_) => {}
            None if first_moves => {}
            None => items.push(MenuItem::OfferDraw),
        }
        items.push(if first_moves {
            MenuItem::Abort
        } else {
            MenuItem::Resign
        });
        items.extend([MenuItem::Flip, MenuItem::Back]);
        items
    }
}

/// The pieces a pawn can become, in the order the prompt offers them.
pub const PROMOTIONS: [Piece; 4] = Piece::PROMOTIONS;

/// How the player is entering their move, and what they've opened, during
/// a game. None of it outlives the game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Desk {
    /// The square the arrows are on, once they've been used.
    pub cursor: Option<Square>,
    /// The piece picked up, to move with Enter on another square.
    pub selected: Option<Square>,
    pub typed: String,
    /// The Esc menu, open at this row.
    pub menu: Option<usize>,
    /// A pawn about to reach the last rank, and the piece the prompt is on.
    pub promoting: Option<(Square, Square, usize)>,
    pub confirm_resign: bool,
    /// The board turned around from how it's usually seen.
    pub flipped: bool,
    /// What went wrong with the last try.
    pub notice: Option<&'static str>,
    /// Moves in the game this belongs to, to notice a new one.
    pub moves: usize,
}

#[derive(Default)]
pub struct ChessUi {
    pub screen: ChessScreen,
    pub prefs: Prefs,
    /// The settings screen's row.
    pub selected: usize,
    pub(crate) desk: Desk,
    /// The challenge this host last asked the room for, until its view
    /// shows it: keys pressed meanwhile step on from there.
    pending: Option<ChessSettings>,
    pub(crate) size: Size,
}

/// The room's view, if it's a chess room.
pub fn chess_view(room: &InRoom) -> Option<&RoomView<Chess>> {
    room.view.of::<Chess>()
}

/// The side the player in `slot` plays in `game`.
pub(crate) fn color_of(game: &GameView, slot: Option<u8>) -> Option<piece::Color> {
    game.players.color_of(u64::from(slot?))
}

/// The side this player plays in the room's game, if they play.
pub(crate) fn my_color(room: &InRoom, game: &GameView) -> Option<piece::Color> {
    color_of(game, room.my_seat()?.slot)
}

/// The player at the bottom of the board, as they see it: their own side,
/// or the side of the friend they watch, else white.
pub(crate) fn bottom(room: &InRoom, game: &GameView, flipped: bool) -> piece::Color {
    let side = color_of(game, room.followed().and_then(|s| s.slot)).unwrap_or(piece::Color::White);
    if flipped { !side } else { side }
}

/// How fast a time control plays, in words.
pub fn speed(control: TimeControl) -> &'static str {
    control.category().name()
}

/// Where the host's side leaves everyone, from `me`'s chair.
fn sides(side: Side, host: &str, hosting: bool) -> String {
    match (side, hosting) {
        (Side::Random, _) => "sides drawn".to_string(),
        (side, true) => format!("you play {}", side.name()),
        (side, false) => format!("{host} plays {}", side.name()),
    }
}

impl ChessUi {
    fn save(&self) -> GameFx {
        GameFx::SavePrefs(self.prefs.to_saved_text())
    }

    /// Forgets how a move was being entered once a new game has begun, or
    /// the room is between games.
    fn sync(&mut self, view: &RoomView<Chess>) {
        let moves = view.game.as_ref().map_or(0, |g| g.moves.len());
        let between = matches!(view.shell.phase, PhaseView::Waiting { .. });
        if between || moves < self.desk.moves {
            self.desk = Desk {
                flipped: self.desk.flipped,
                ..Desk::default()
            };
        }
        self.desk.moves = moves;
    }

    /// What the host's next change starts from: what they last asked for,
    /// or else what the room plays.
    fn challenge_now(&mut self, view: &RoomView<Chess>) -> ChessSettings {
        let lobby = matches!(view.shell.phase, PhaseView::Waiting { .. });
        if !lobby || self.pending == Some(view.settings) {
            self.pending = None;
        }
        self.pending.unwrap_or(view.settings)
    }

    /// The host sets the room's challenge to `settings`, which they keep
    /// for the rooms they make.
    fn challenge(&mut self, settings: ChessSettings) -> Reply {
        self.pending = Some(settings);
        self.prefs.challenge = settings;
        Reply {
            fx: vec![GameFx::Settings(encode(&settings)), self.save()],
            ..Reply::default()
        }
    }

    fn lobby_key(&mut self, key: Key, room: &InRoom, view: &RoomView<Chess>) -> Option<Reply> {
        match key {
            Key::Char('e') => {
                self.screen = ChessScreen::Challenge { selected: 0 };
                Some(Reply::to(Next::Game))
            }
            // The host steps through the presets.
            Key::Left | Key::Right | Key::Char('a' | 'h' | 'd' | 'l') if room.hosting() => {
                let back = matches!(key, Key::Left | Key::Char('a' | 'h'));
                let settings = self.challenge_now(view);
                let now = settings.control;
                let at = PRESETS.iter().position(|&p| p == now);
                let next = match (at, back) {
                    (Some(i), true) => i.checked_sub(1),
                    (Some(i), false) => Some(i + 1).filter(|&i| i < PRESETS.len()),
                    // From a time of their own, to the nearest preset.
                    (None, _) => Some(
                        PRESETS
                            .iter()
                            .position(|p| p.estimated() >= now.estimated())
                            .unwrap_or(PRESETS.len() - 1),
                    ),
                };
                Some(match next {
                    Some(i) => self.challenge(ChessSettings {
                        control: PRESETS[i],
                        ..settings
                    }),
                    None => Reply::default(),
                })
            }
            _ => None,
        }
    }

    /// Moves the cursor a square across the screen: `right` and `up` as
    /// the board is drawn, with `bottom` at the bottom.
    fn step_cursor(&mut self, right: i8, up: i8, bottom: piece::Color, me: piece::Color) {
        let (right, up) = match bottom {
            piece::Color::White => (right, up),
            piece::Color::Black => (-right, -up),
        };
        let cursor = match self.desk.cursor {
            // Arrows start on the king's pawn.
            None => Square::new(4, if me == piece::Color::White { 1 } else { 6 }),
            Some(at) => Square::new(
                (at.file() as i8 + right).clamp(0, 7) as u8,
                (at.rank() as i8 + up).clamp(0, 7) as u8,
            ),
        };
        self.desk.cursor = cursor;
    }

    /// Sends `mv`, and forgets how it was entered.
    fn play(&mut self, mv: Move) -> Reply {
        self.desk.typed.clear();
        self.desk.selected = None;
        self.desk.promoting = None;
        Reply::fx(GameFx::Play(encode(&ChessInput::Move(mv))))
    }

    fn entered(&mut self, entered: Entered) -> Reply {
        match entered {
            Entered::Play(mv) => self.play(mv),
            Entered::Promote { from, to } => {
                self.desk.promoting = Some((from, to, 0));
                Reply::default()
            }
            Entered::Refused(why) => {
                self.desk.notice = Some(why);
                Reply::default()
            }
        }
    }

    /// Enter on the cursor's square: picks up one of `me`'s pieces, or
    /// moves the one picked up there.
    fn pick(&mut self, position: &Position, me: piece::Color) -> Reply {
        let Some(at) = self.desk.cursor else {
            return Reply::default();
        };
        let mine = position.piece_at(at).is_some_and(|(c, _)| c == me);
        match self.desk.selected {
            Some(from) if from == at => self.desk.selected = None,
            _ if mine => self.desk.selected = Some(at),
            Some(_) if position.side_to_move() != me => self.desk.notice = Some(NOT_YET),
            Some(from) => {
                let moves = entry::between(position, from, at);
                let entered = entry::choose(&moves, self.prefs.auto_queen)
                    .unwrap_or(Entered::Refused("That piece can't go there."));
                if matches!(entered, Entered::Refused(_)) {
                    self.desk.selected = None;
                }
                return self.entered(entered);
            }
            None => {}
        }
        Reply::default()
    }

    /// A key while `me` plays `game`, which goes on.
    fn play_key(&mut self, key: Key, game: &GameView, me: piece::Color) -> Reply {
        let Some((position, _)) = game.replay() else {
            return Reply::default();
        };
        self.desk.notice = None;
        if let Some((from, to, at)) = self.desk.promoting {
            let chosen = match key.lower() {
                Key::Char('q') => Some(0),
                Key::Char('r') => Some(1),
                Key::Char('b') => Some(2),
                Key::Char('n') => Some(3),
                Key::Enter | Key::Char(' ') => Some(at),
                Key::Left | Key::Up => {
                    self.desk.promoting = Some((from, to, (at + 3) % 4));
                    None
                }
                Key::Right | Key::Down => {
                    self.desk.promoting = Some((from, to, (at + 1) % 4));
                    None
                }
                _ => {
                    self.desk.promoting = None;
                    None
                }
            };
            return match chosen {
                Some(i) => self.play(Move {
                    from,
                    to,
                    promotion: Some(PROMOTIONS[i]),
                }),
                None => Reply::default(),
            };
        }
        if self.desk.confirm_resign {
            self.desk.confirm_resign = false;
            return match key.lower() {
                Key::Char('y') => Reply::fx(GameFx::Play(encode(&ChessInput::Resign))),
                _ => Reply::default(),
            };
        }
        if let Some(row) = self.desk.menu {
            return self.menu_key(key, row, game, me);
        }
        let bottom = bottom_of(me, self.desk.flipped);
        let arrow = |key: Key| match key {
            Key::Up => Some((0, 1)),
            Key::Down => Some((0, -1)),
            Key::Left => Some((-1, 0)),
            Key::Right => Some((1, 0)),
            _ => None,
        };
        let wasd = |key: Key| match key.lower() {
            Key::Char('w' | 'k') => Some((0, 1)),
            Key::Char('s' | 'j') => Some((0, -1)),
            Key::Char('a' | 'h') => Some((-1, 0)),
            Key::Char('d' | 'l') => Some((1, 0)),
            _ => None,
        };
        let cursor_keys = self.prefs.input == MoveInput::Cursor;
        if let Some((right, up)) = arrow(key).or(wasd(key).filter(|_| cursor_keys)) {
            self.step_cursor(right, up, bottom, me);
            return Reply::default();
        }
        match key {
            Key::Esc => self.desk.menu = Some(0),
            Key::Enter | Key::Char(' ') if !self.desk.typed.is_empty() => {
                if position.side_to_move() != me {
                    self.desk.notice = Some(NOT_YET);
                } else {
                    let entered = entry::enter(&position, &self.desk.typed, self.prefs.auto_queen);
                    return self.entered(entered);
                }
            }
            Key::Enter | Key::Char(' ') => return self.pick(&position, me),
            Key::Backspace => {
                self.desk.typed.pop();
            }
            Key::Char(c)
                if !cursor_keys && entry::typable(c) && self.desk.typed.len() < MAX_TYPED =>
            {
                self.desk.typed.push(c);
                self.desk.selected = None;
            }
            _ => {}
        }
        Reply::default()
    }

    fn menu_key(&mut self, key: Key, row: usize, game: &GameView, me: piece::Color) -> Reply {
        let items = MenuItem::offered(game, me);
        let row = row.min(items.len() - 1);
        if let Some(row) = key.lower().step(row, items.len()) {
            self.desk.menu = Some(row);
            return Reply::default();
        }
        match key.lower() {
            Key::Esc | Key::Char('q') => self.desk.menu = None,
            Key::Enter | Key::Char(' ') => {
                self.desk.menu = None;
                let input = match items[row] {
                    MenuItem::OfferDraw => ChessInput::OfferDraw,
                    MenuItem::AcceptDraw => ChessInput::AcceptDraw,
                    MenuItem::DeclineDraw => ChessInput::DeclineDraw,
                    MenuItem::Abort => ChessInput::Abort,
                    MenuItem::Resign if self.prefs.confirm_resign => {
                        self.desk.confirm_resign = true;
                        return Reply::default();
                    }
                    MenuItem::Resign => ChessInput::Resign,
                    MenuItem::Flip => {
                        self.desk.flipped = !self.desk.flipped;
                        return Reply::default();
                    }
                    MenuItem::Back => return Reply::default(),
                };
                return Reply::fx(GameFx::Play(encode(&input)));
            }
            _ => {}
        }
        Reply::default()
    }

    fn settings_key(&mut self, key: Key) -> Reply {
        let rows = ui::SETTINGS.len();
        if let Some(selected) = key.step(self.selected, rows) {
            self.selected = selected;
            return Reply::default();
        }
        let back = matches!(key, Key::Left | Key::Char('a' | 'h'));
        match key {
            Key::Esc | Key::Char('q') => Reply::to(Next::Home),
            Key::Left | Key::Right | Key::Enter | Key::Char(' ' | 'a' | 'h' | 'd' | 'l') => {
                ui::SETTINGS[self.selected].change(&mut self.prefs, back);
                Reply::fx(self.save())
            }
            _ => Reply::default(),
        }
    }

    fn challenge_key(&mut self, selected: usize, key: Key, room: Option<&InRoom>) -> Reply {
        let Some((room, view)) = room.and_then(|r| Some((r, chess_view(r)?))) else {
            return Reply::to(Next::Room);
        };
        if let Some(selected) = key.step(selected, 3) {
            self.screen = ChessScreen::Challenge { selected };
            return Reply::default();
        }
        let back = matches!(key, Key::Left | Key::Char('a' | 'h'));
        match key {
            Key::Esc | Key::Char('q') | Key::Enter => Reply::to(Next::Room),
            Key::Left | Key::Right | Key::Char('a' | 'h' | 'd' | 'l') if room.hosting() => {
                let was = self.challenge_now(view);
                let mut settings = was;
                match selected {
                    0 => settings.control = prefs::step_base(settings.control, back),
                    1 => settings.control = prefs::step_increment(settings.control, back),
                    _ => settings.side = prefs::step_side(settings.side, back),
                }
                if settings == was {
                    return Reply::default();
                }
                self.challenge(settings)
            }
            _ => Reply::default(),
        }
    }
}

/// The side at the bottom of `me`'s board.
fn bottom_of(me: piece::Color, flipped: bool) -> piece::Color {
    if flipped { !me } else { me }
}

const NOT_YET: &str = "Wait for your move.";

impl GameUi for ChessUi {
    fn id(&self) -> GameId {
        GameId::Chess
    }

    fn title(&self) -> &'static str {
        "Chess"
    }

    fn logo(&self) -> (&'static [&'static str], Color) {
        (&ui::LOGO, Color::LightCyan)
    }

    fn home_items(&self) -> &'static [&'static str] {
        &["Board and pieces"]
    }

    fn open(&mut self, _: usize) {
        self.screen = ChessScreen::Settings;
    }

    fn room_note(&self) -> Option<String> {
        let challenge = self.prefs.challenge;
        Some(format!(
            "{} {}",
            challenge.control,
            speed(challenge.control)
        ))
    }

    fn room_settings(&self) -> Vec<u8> {
        encode(&self.prefs.challenge)
    }

    fn quick_play(&self, _: Size) -> Result<QuickPlay, String> {
        Err("Quick pairing for chess is on its way. Make a private room for now, and invite a friend or share its code.".to_string())
    }

    fn ratings(&self) -> &'static [Category] {
        &RATINGS
    }

    fn key(&mut self, key: Key, ctx: &Ctx) -> Reply {
        let key = key.lower();
        match self.screen {
            ChessScreen::Settings => self.settings_key(key),
            ChessScreen::Challenge { selected } => self.challenge_key(selected, key, ctx.room),
        }
    }

    fn resize(&mut self, size: Size) {
        self.size = size;
    }

    fn draw(&self, frame: &mut Frame, ctx: &Ctx) {
        ui::draw(frame, self, ctx);
    }

    fn room_key(&mut self, key: Key, room: &InRoom) -> Option<Reply> {
        let view = chess_view(room)?;
        self.sync(view);
        let flip = matches!(key, Key::Char('f' | 'F'));
        match &view.shell.phase {
            PhaseView::Waiting { .. } => self.lobby_key(key.lower(), room, view),
            PhaseView::Running { .. } => {
                let game = view.game.as_ref()?;
                match my_color(room, game) {
                    Some(me) if game.outcome.is_none() => Some(self.play_key(key, game, me)),
                    // Watching.
                    _ if flip => {
                        self.desk.flipped = !self.desk.flipped;
                        Some(Reply::default())
                    }
                    _ => None,
                }
            }
            PhaseView::Countdown { .. } | PhaseView::Results { .. } if flip => {
                self.desk.flipped = !self.desk.flipped;
                Some(Reply::default())
            }
            _ => None,
        }
    }

    fn settings_note(&self, old: &dyn AnyView, room: &InRoom) -> Option<String> {
        let (old, now) = (old.of::<Chess>()?, chess_view(room)?);
        let (was, is) = (old.settings, now.settings);
        let host = now.shell.host().map_or("The host", |h| h.who.name.as_str());
        if was.control != is.control {
            Some(format!(
                "{host} made it {} {}",
                is.control,
                speed(is.control)
            ))
        } else if was.side != is.side {
            Some(format!("Now {}", sides(is.side, host, false)))
        } else {
            None
        }
    }

    fn lobby_line(&self, room: &InRoom) -> Vec<Span<'static>> {
        let Some(view) = chess_view(room) else {
            return Vec::new();
        };
        let settings = view.settings;
        let dim = shell_tui::ui::dim();
        let control = format!("{} {}", settings.control, speed(settings.control));
        let control = if room.hosting() {
            Span::styled(
                format!("◂ {control} ▸"),
                Style::default().fg(Color::LightYellow),
            )
        } else {
            Span::styled(control, dim)
        };
        let host = view
            .shell
            .host()
            .map_or("the host", |h| h.who.name.as_str());
        vec![
            control,
            Span::styled(
                format!(" · {}", sides(settings.side, host, room.hosting())),
                dim,
            ),
        ]
    }

    fn lobby_keys(&self) -> Option<&'static str> {
        Some("e challenge")
    }

    fn host_keys(&self) -> Option<&'static str> {
        Some("←→ time control")
    }

    fn draw_round(&self, frame: &mut Frame, room: &InRoom, ctx: &Ctx) {
        ui::draw_round(frame, self, room, ctx);
    }

    fn result_note(&self, room: &InRoom) -> Option<String> {
        let game = chess_view(room)?.game.as_ref()?;
        Some(game.outcome?.to_string())
    }

    fn score(&self, room: &InRoom, who: PlayerId) -> String {
        let Some(view) = chess_view(room) else {
            return String::new();
        };
        let slot = view
            .shell
            .seats
            .iter()
            .find(|s| s.who.id == who)
            .and_then(|s| s.slot);
        let points = view
            .game
            .as_ref()
            .and_then(|g| g.outcome?.score(color_of(g, slot)?));
        match points {
            Some(p) if p > 0.75 => "1",
            Some(p) if p > 0.25 => "½",
            Some(_) => "0",
            None => "",
        }
        .to_string()
    }

    fn restore(&mut self, saved: Saved) {
        self.prefs = Prefs::from_saved_text(&saved.prefs);
    }

    fn saved(&self) -> Saved {
        Saved {
            prefs: self.prefs.to_saved_text(),
            records: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chess_core::control::Category as Speed;

    use super::*;

    #[test]
    fn ratings_are_kept_by_speed() {
        let ids: Vec<&str> = RATINGS.iter().map(|c| c.id).collect();
        let speeds: Vec<&str> = Speed::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(ids, speeds);
    }
}
