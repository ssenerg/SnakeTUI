//! Time controls, the presets quick pairing offers, and the categories
//! ratings are kept in.

use std::fmt;
use std::time::Duration;

/// Each side's starting time, and the Fischer increment added after each
/// of their moves.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TimeControl {
    pub base: Duration,
    pub increment: Duration,
}

/// The longest base a private challenge may set.
pub const MAX_BASE: Duration = Duration::from_secs(180 * 60);
/// The largest increment a private challenge may set.
pub const MAX_INCREMENT: Duration = Duration::from_secs(180);

/// What quick pairing offers, quickest first.
pub const PRESETS: [TimeControl; 11] = [
    TimeControl::minutes(1, 0),
    TimeControl::minutes(2, 1),
    TimeControl::minutes(3, 0),
    TimeControl::minutes(3, 2),
    TimeControl::minutes(5, 0),
    TimeControl::minutes(5, 3),
    TimeControl::minutes(10, 0),
    TimeControl::minutes(10, 5),
    TimeControl::minutes(15, 10),
    TimeControl::minutes(30, 0),
    TimeControl::minutes(30, 20),
];

/// [`PRESETS`] as written: what quick pairing's pools are called.
pub const PRESET_NAMES: [&str; 11] = [
    "1+0", "2+1", "3+0", "3+2", "5+0", "5+3", "10+0", "10+5", "15+10", "30+0", "30+20",
];

impl TimeControl {
    /// `base` minutes and `increment` seconds, as in "3+2".
    pub const fn minutes(base: u64, increment: u64) -> TimeControl {
        TimeControl {
            base: Duration::from_secs(base * 60),
            increment: Duration::from_secs(increment),
        }
    }

    /// Whether a game can be played with it: some base time, up to
    /// [`MAX_BASE`], and an increment up to [`MAX_INCREMENT`].
    pub fn is_playable(self) -> bool {
        !self.base.is_zero() && self.base <= MAX_BASE && self.increment <= MAX_INCREMENT
    }

    /// How long a side's clock is expected to give them over a 40-move
    /// game: the base and 40 increments.
    pub fn estimated(self) -> Duration {
        self.base + self.increment * 40
    }

    pub fn category(self) -> Category {
        match self.estimated().as_secs_f64() {
            s if s < 180.0 => Category::Bullet,
            s if s < 480.0 => Category::Blitz,
            s if s < 1500.0 => Category::Rapid,
            _ => Category::Classical,
        }
    }
}

/// Minutes and seconds plus, as in "3+2", or "0.5+0" for half a minute.
impl fmt::Display for TimeControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let minutes = trimmed(self.base.as_secs_f64() / 60.0);
        let increment = trimmed(self.increment.as_secs_f64());
        write!(f, "{minutes}+{increment}")
    }
}

/// `value` to two decimal places, without trailing zeros.
fn trimmed(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// How fast a game is, by [`TimeControl::estimated`]: under 3 minutes is
/// bullet, under 8 blitz, under 25 rapid, and anything longer classical.
/// Each player has a rating in each.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Category {
    Bullet,
    Blitz,
    Rapid,
    Classical,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Bullet,
        Category::Blitz,
        Category::Rapid,
        Category::Classical,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Bullet => "bullet",
            Category::Blitz => "blitz",
            Category::Rapid => "rapid",
            Category::Classical => "classical",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_named_as_written() {
        let written: Vec<String> = PRESETS.iter().map(|p| p.to_string()).collect();
        assert_eq!(written, PRESET_NAMES);
    }

    fn secs(base: u64, increment: u64) -> TimeControl {
        TimeControl {
            base: Duration::from_secs(base),
            increment: Duration::from_secs(increment),
        }
    }

    #[test]
    fn categories_change_at_3_8_and_25_minutes() {
        assert_eq!(secs(179, 0).category(), Category::Bullet);
        assert_eq!(secs(180, 0).category(), Category::Blitz);
        assert_eq!(secs(479, 0).category(), Category::Blitz);
        assert_eq!(secs(480, 0).category(), Category::Rapid);
        assert_eq!(secs(1499, 0).category(), Category::Rapid);
        assert_eq!(secs(1500, 0).category(), Category::Classical);
        // Increments count forty times over.
        assert_eq!(secs(19, 4).category(), Category::Bullet);
        assert_eq!(secs(20, 4).category(), Category::Blitz);
        assert_eq!(secs(60, 30).category(), Category::Rapid);
        let just_under = TimeControl {
            base: Duration::from_millis(179_999),
            increment: Duration::ZERO,
        };
        assert_eq!(just_under.category(), Category::Bullet);
    }

    #[test]
    fn presets_fall_where_expected() {
        let categories: Vec<(String, Category)> = PRESETS
            .iter()
            .map(|tc| (tc.to_string(), tc.category()))
            .collect();
        let expected = [
            ("1+0", Category::Bullet),
            ("2+1", Category::Bullet),
            ("3+0", Category::Blitz),
            ("3+2", Category::Blitz),
            ("5+0", Category::Blitz),
            ("5+3", Category::Blitz),
            ("10+0", Category::Rapid),
            ("10+5", Category::Rapid),
            ("15+10", Category::Rapid),
            ("30+0", Category::Classical),
            ("30+20", Category::Classical),
        ];
        let expected: Vec<(String, Category)> = expected
            .into_iter()
            .map(|(name, category)| (name.to_string(), category))
            .collect();
        assert_eq!(categories, expected);
        assert!(PRESETS.iter().all(|tc| tc.is_playable()));
    }

    #[test]
    fn shows_part_minutes() {
        assert_eq!(secs(30, 0).to_string(), "0.5+0");
        assert_eq!(secs(90, 1).to_string(), "1.5+1");
        assert_eq!(secs(15, 0).to_string(), "0.25+0");
    }

    #[test]
    fn some_controls_cant_be_played() {
        assert!(!secs(0, 0).is_playable());
        assert!(!secs(0, 2).is_playable());
        assert!(secs(15, 0).is_playable());
        assert!(!TimeControl::minutes(181, 0).is_playable());
        assert!(!secs(60, 181).is_playable());
        assert!(TimeControl::minutes(180, 180).is_playable());
    }
}
