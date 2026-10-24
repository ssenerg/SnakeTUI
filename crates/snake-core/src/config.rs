//! Game settings: the [`Rules`] that change how a game plays, and the
//! [`Theme`] that only changes how it looks.

/// Names of the selectable colors. The UI maps each index to a terminal
/// color, in this order.
pub const COLOR_NAMES: &[&str] = &[
    "red",
    "light red",
    "green",
    "light green",
    "yellow",
    "light yellow",
    "blue",
    "light blue",
    "magenta",
    "light magenta",
    "cyan",
    "light cyan",
    "white",
    "gray",
    "dark gray",
    "black",
];

pub fn color_name(idx: usize) -> &'static str {
    COLOR_NAMES[idx % COLOR_NAMES.len()]
}

/// Selectable icons for food items, as `(name, glyph)`. Only single-width
/// characters — the board renders each cell as the glyph plus a space.
pub const GLYPHS: &[(&str, &str)] = &[
    ("dot", "●"),
    ("circle", "○"),
    ("diamond", "◆"),
    ("diamond outline", "◇"),
    ("star", "★"),
    ("star outline", "☆"),
    ("heart", "♥"),
    ("square", "■"),
    ("square outline", "□"),
    ("triangle up", "▲"),
    ("triangle down", "▼"),
    ("cross", "✖"),
    ("plus", "✚"),
    ("asterisk", "*"),
    ("at sign", "@"),
    ("dollar", "$"),
    ("hash", "#"),
    ("percent", "%"),
];

pub fn glyph_name(idx: usize) -> &'static str {
    GLYPHS[idx % GLYPHS.len()].0
}

pub fn glyph_char(idx: usize) -> &'static str {
    GLYPHS[idx % GLYPHS.len()].1
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WallMode {
    Full,
    Open,
    Gaps,
}

impl WallMode {
    pub const ALL: [WallMode; 3] = [WallMode::Full, WallMode::Open, WallMode::Gaps];
    pub(crate) const LABELS: [&'static str; 3] =
        ["full_edge_wall", "no_edge_wall", "some_edge_wall"];
    pub(crate) const DISPLAY: [&'static str; 3] = ["solid", "none (wrap)", "gaps"];

    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap()
    }

    pub fn label(self) -> &'static str {
        Self::LABELS[self.index()]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum InnerWalls {
    Off,
    Few,
    Medium,
    Many,
}

impl InnerWalls {
    pub const ALL: [InnerWalls; 4] = [
        InnerWalls::Off,
        InnerWalls::Few,
        InnerWalls::Medium,
        InnerWalls::Many,
    ];
    pub(crate) const LABELS: [&'static str; 4] = ["off", "few", "medium", "many"];

    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap()
    }

    pub fn label(self) -> &'static str {
        Self::LABELS[self.index()]
    }

    pub fn density_percent(self) -> usize {
        match self {
            InnerWalls::Off => 0,
            InnerWalls::Few => 6,
            InnerWalls::Medium => 12,
            InnerWalls::Many => 18,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum GrowthMode {
    Instant,
    AtTail,
}

impl GrowthMode {
    pub const ALL: [GrowthMode; 2] = [GrowthMode::Instant, GrowthMode::AtTail];
    pub(crate) const LABELS: [&'static str; 2] = ["instant", "at_tail"];
    pub(crate) const DISPLAY: [&'static str; 2] = ["instant", "when food reaches tail"];

    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap()
    }

    pub fn label(self) -> &'static str {
        Self::DISPLAY[self.index()]
    }
}

/// When a direction key moves the snake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MoveTiming {
    /// Right away, ahead of the tick timer.
    Instant,
    /// On the next tick; keys pressed in between are queued.
    OnTick,
}

impl MoveTiming {
    pub const ALL: [MoveTiming; 2] = [MoveTiming::Instant, MoveTiming::OnTick];
    pub(crate) const LABELS: [&'static str; 2] = ["instant", "tick"];
    pub(crate) const DISPLAY: [&'static str; 2] = ["instant", "on tick"];

    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap()
    }
}

#[derive(Clone, Copy)]
pub enum ColorSlot {
    Head,
    Body,
    Food,
    SuperFood,
    Poison,
    Wall,
    Background,
}

pub const COLOR_SLOTS: usize = 7;

#[derive(Clone, Copy)]
pub enum IconSlot {
    Food,
    SuperFood,
    Poison,
}

pub const ICON_SLOTS: usize = 3;

impl IconSlot {
    /// The color slot this icon is drawn with, used for menu previews.
    pub fn color_slot(self) -> ColorSlot {
        match self {
            IconSlot::Food => ColorSlot::Food,
            IconSlot::SuperFood => ColorSlot::SuperFood,
            IconSlot::Poison => ColorSlot::Poison,
        }
    }
}

/// Everything that changes how a game plays. Two games with equal rules
/// count toward the same record.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rules {
    pub width: u16,
    pub height: u16,
    pub tick_ms: u64,
    pub wall_mode: WallMode,
    pub inner_walls: InnerWalls,
    pub food_interval_s: u64,
    pub food_ttl_s: u64,
    pub super_interval_s: u64,
    pub super_ttl_s: u64,
    pub poison_interval_s: u64,
    pub poison_ttl_s: u64,
    pub growth_mode: GrowthMode,
    pub pass_through_self: bool,
    pub super_growth: u32,
    pub boost_duration_s: u64,
    pub move_timing: MoveTiming,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            width: 30,
            height: 20,
            tick_ms: 150,
            wall_mode: WallMode::Full,
            inner_walls: InnerWalls::Off,
            food_interval_s: 3,
            food_ttl_s: 15,
            super_interval_s: 20,
            super_ttl_s: 8,
            poison_interval_s: 15,
            poison_ttl_s: 12,
            growth_mode: GrowthMode::Instant,
            pass_through_self: false,
            super_growth: 3,
            boost_duration_s: 5,
            move_timing: MoveTiming::Instant,
        }
    }
}

impl Rules {
    /// Identifies these rules in the records file. The format is stored on
    /// disk, so changing it orphans existing records. Settings added later
    /// are appended only when they differ from the old behaviour.
    pub fn record_key(&self) -> String {
        let mut key = format!(
            "size:{}x{} tick:{} edge:{} inner:{} food:{}/{} super:{}/{} poison:{}/{} growth:{} ghost:{} supergrow:{} boost:{}",
            self.width,
            self.height,
            self.tick_ms,
            self.wall_mode.label(),
            self.inner_walls.label(),
            self.food_interval_s,
            self.food_ttl_s,
            self.super_interval_s,
            self.super_ttl_s,
            self.poison_interval_s,
            self.poison_ttl_s,
            self.growth_mode.label(),
            self.pass_through_self,
            self.super_growth,
            self.boost_duration_s,
        );
        if self.move_timing == MoveTiming::OnTick {
            key.push_str(" timing:tick");
        }
        key
    }
}

/// How the game looks: icon and color choices, as indices into [`GLYPHS`]
/// and [`COLOR_NAMES`]. Never affects records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub icons: [usize; ICON_SLOTS],
    pub colors: [usize; COLOR_SLOTS],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            // food ●, super food ★, poison ✖
            icons: [0, 4, 11],
            // head, body, food, super food, poison, wall, background
            colors: [3, 2, 1, 5, 8, 13, 15],
        }
    }
}

impl Theme {
    pub fn icon(&self, slot: IconSlot) -> &'static str {
        glyph_char(self.icons[slot as usize])
    }

    pub fn color_index(&self, slot: ColorSlot) -> usize {
        self.colors[slot as usize] % COLOR_NAMES.len()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameConfig {
    pub rules: Rules,
    pub theme: Theme,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_record_key_is_unchanged() {
        // Saved records are keyed by this string; it must never drift.
        assert_eq!(
            Rules::default().record_key(),
            "size:30x20 tick:150 edge:full_edge_wall inner:off food:3/15 super:20/8 \
             poison:15/12 growth:instant ghost:false supergrow:3 boost:5"
        );
    }

    #[test]
    fn on_tick_games_have_their_own_records() {
        let on_tick = Rules {
            move_timing: MoveTiming::OnTick,
            ..Rules::default()
        };
        assert_eq!(
            on_tick.record_key(),
            Rules::default().record_key() + " timing:tick"
        );
    }
}
