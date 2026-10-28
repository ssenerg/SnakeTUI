//! What's kept for a chess player: how the board looks, how moves are
//! entered, and the challenge their private rooms start with. Saved as
//! `key=value` lines; unknown keys and bad values are skipped.

use std::time::Duration;

use chess_core::control::{MAX_BASE, MAX_INCREMENT, TimeControl};
use chess_core::round::{ChessSettings, Side};

/// The squares' colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BoardLook {
    #[default]
    Wood,
    Green,
    Blue,
    Gray,
}

impl BoardLook {
    pub const ALL: [BoardLook; 4] = [
        BoardLook::Wood,
        BoardLook::Green,
        BoardLook::Blue,
        BoardLook::Gray,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BoardLook::Wood => "wood",
            BoardLook::Green => "green",
            BoardLook::Blue => "blue",
            BoardLook::Gray => "gray",
        }
    }
}

/// How pieces are drawn. Some terminals show chess glyphs as emoji, or
/// too wide; letters always line up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PieceStyle {
    #[default]
    Glyphs,
    Letters,
}

impl PieceStyle {
    pub const ALL: [PieceStyle; 2] = [PieceStyle::Glyphs, PieceStyle::Letters];

    pub fn name(self) -> &'static str {
        match self {
            PieceStyle::Glyphs => "glyphs",
            PieceStyle::Letters => "letters",
        }
    }
}

/// What letter keys do during a game. Arrows and Enter pick squares
/// either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MoveInput {
    /// They type moves, such as `Nf3` or `g1f3`.
    #[default]
    Typed,
    /// WASD and HJKL move the cursor, like the arrows.
    Cursor,
}

impl MoveInput {
    pub const ALL: [MoveInput; 2] = [MoveInput::Typed, MoveInput::Cursor];

    pub fn name(self) -> &'static str {
        match self {
            MoveInput::Typed => "typed",
            MoveInput::Cursor => "cursor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub board: BoardLook,
    pub pieces: PieceStyle,
    /// Files and ranks along the board's edges.
    pub coordinates: bool,
    /// Pawns reaching the last rank become queens without asking.
    pub auto_queen: bool,
    pub input: MoveInput,
    /// Resigning asks first.
    pub confirm_resign: bool,
    /// What a private room this player makes plays by. A host's changes in
    /// the lobby are kept here too.
    pub challenge: ChessSettings,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            board: BoardLook::default(),
            pieces: PieceStyle::default(),
            coordinates: true,
            auto_queen: false,
            input: MoveInput::default(),
            confirm_resign: true,
            challenge: ChessSettings::default(),
        }
    }
}

/// The value named `text` among `all`.
fn named<T: Copy>(all: &[T], name: impl Fn(T) -> &'static str, text: &str) -> Option<T> {
    all.iter().copied().find(|&v| name(v) == text)
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

impl Prefs {
    pub fn to_saved_text(&self) -> String {
        let control = self.challenge.control;
        [
            ("board", self.board.name().to_string()),
            ("pieces", self.pieces.name().to_string()),
            ("coordinates", on_off(self.coordinates).to_string()),
            ("auto_queen", on_off(self.auto_queen).to_string()),
            ("input", self.input.name().to_string()),
            ("confirm_resign", on_off(self.confirm_resign).to_string()),
            ("base_s", control.base.as_secs().to_string()),
            ("increment_s", control.increment.as_secs().to_string()),
            ("side", self.challenge.side.name().to_string()),
        ]
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect()
    }

    /// Reads [`Self::to_saved_text`]'s text; empty text is the defaults.
    pub fn from_saved_text(text: &str) -> Self {
        let mut prefs = Self::default();
        let flag = |value: &str| match value {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let secs = || value.parse().ok().map(Duration::from_secs);
            match key.trim() {
                "board" => set(
                    &mut prefs.board,
                    named(&BoardLook::ALL, BoardLook::name, value),
                ),
                "pieces" => set(
                    &mut prefs.pieces,
                    named(&PieceStyle::ALL, PieceStyle::name, value),
                ),
                "coordinates" => set(&mut prefs.coordinates, flag(value)),
                "auto_queen" => set(&mut prefs.auto_queen, flag(value)),
                "input" => set(
                    &mut prefs.input,
                    named(&MoveInput::ALL, MoveInput::name, value),
                ),
                "confirm_resign" => set(&mut prefs.confirm_resign, flag(value)),
                "base_s" => set(
                    &mut prefs.challenge.control.base,
                    secs().filter(|d| !d.is_zero() && *d <= MAX_BASE),
                ),
                "increment_s" => set(
                    &mut prefs.challenge.control.increment,
                    secs().filter(|d| *d <= MAX_INCREMENT),
                ),
                "side" => set(
                    &mut prefs.challenge.side,
                    named(&Side::ALL, Side::name, value),
                ),
                _ => {}
            }
        }
        prefs
    }
}

fn set<T>(slot: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *slot = value;
    }
}

/// Starting times a challenge steps through, in seconds: a quarter of a
/// minute up to three hours.
const BASES: [u64; 27] = [
    15, 30, 45, 60, 90, 120, 180, 240, 300, 360, 420, 480, 540, 600, 900, 1200, 1500, 1800, 2400,
    2700, 3600, 4500, 5400, 6300, 7200, 9000, 10800,
];

/// Increments a challenge steps through, in seconds.
const INCREMENTS: [u64; 31] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 25, 30, 35, 40, 45,
    60, 90, 120, 150, 180,
];

/// The step after `now` on `steps`, or before it when `down`, stopping at
/// either end. A value between steps goes to the next one either way.
fn step(steps: &[u64], now: u64, down: bool) -> u64 {
    let at = steps.partition_point(|&s| s < now);
    let exact = steps.get(at) == Some(&now);
    let i = match (down, exact) {
        (true, _) => at.saturating_sub(1),
        (false, true) => (at + 1).min(steps.len() - 1),
        (false, false) => at.min(steps.len() - 1),
    };
    steps[i]
}

/// A longer or shorter starting time than `control`'s.
pub fn step_base(control: TimeControl, down: bool) -> TimeControl {
    TimeControl {
        base: Duration::from_secs(step(&BASES, control.base.as_secs(), down)),
        ..control
    }
}

/// A bigger or smaller increment than `control`'s.
pub fn step_increment(control: TimeControl, down: bool) -> TimeControl {
    TimeControl {
        increment: Duration::from_secs(step(&INCREMENTS, control.increment.as_secs(), down)),
        ..control
    }
}

/// The side after `side` in [`Side::ALL`], or before it when `back`.
pub fn step_side(side: Side, back: bool) -> Side {
    let i = Side::ALL.iter().position(|&s| s == side).unwrap_or(0);
    let n = Side::ALL.len();
    Side::ALL[if back { (i + n - 1) % n } else { (i + 1) % n }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip_and_bad_lines_are_skipped() {
        let prefs = Prefs {
            board: BoardLook::Blue,
            pieces: PieceStyle::Letters,
            coordinates: false,
            auto_queen: true,
            input: MoveInput::Cursor,
            confirm_resign: false,
            challenge: ChessSettings {
                control: TimeControl::minutes(3, 2),
                side: Side::Black,
            },
        };
        assert_eq!(Prefs::from_saved_text(&prefs.to_saved_text()), prefs);
        assert_eq!(Prefs::from_saved_text(""), Prefs::default());
        let odd =
            Prefs::from_saved_text("board=plaid\nbase_s=0\nincrement_s=x\nnoise\npieces=letters");
        assert_eq!(
            odd,
            Prefs {
                pieces: PieceStyle::Letters,
                ..Prefs::default()
            }
        );
    }

    #[test]
    fn time_controls_step_and_stop_at_the_ends() {
        let tc = TimeControl::minutes(10, 0);
        assert_eq!(step_base(tc, false), TimeControl::minutes(15, 0));
        assert_eq!(step_base(tc, true), TimeControl::minutes(9, 0));
        assert_eq!(step_increment(tc, true), tc, "no less than none");
        let odd = TimeControl {
            base: Duration::from_secs(100),
            increment: Duration::ZERO,
        };
        assert_eq!(step_base(odd, false).base.as_secs(), 120);
        assert_eq!(step_base(odd, true).base.as_secs(), 90);
        let longest = TimeControl::minutes(180, 180);
        assert_eq!(step_base(longest, false), longest);
        assert_eq!(step_increment(longest, false), longest);
        assert_eq!(step_side(Side::Black, false), Side::Random);
        assert_eq!(step_side(Side::Random, true), Side::Black);
    }
}
