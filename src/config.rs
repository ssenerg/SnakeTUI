use std::fs;
use std::path::PathBuf;

use ratatui::style::Color;

pub fn data_dir() -> PathBuf {
    let dir = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".snaketui");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub const PALETTE: &[(&str, Color)] = &[
    ("red", Color::Red),
    ("light red", Color::LightRed),
    ("green", Color::Green),
    ("light green", Color::LightGreen),
    ("yellow", Color::Yellow),
    ("light yellow", Color::LightYellow),
    ("blue", Color::Blue),
    ("light blue", Color::LightBlue),
    ("magenta", Color::Magenta),
    ("light magenta", Color::LightMagenta),
    ("cyan", Color::Cyan),
    ("light cyan", Color::LightCyan),
    ("white", Color::White),
    ("gray", Color::Gray),
    ("dark gray", Color::DarkGray),
    ("black", Color::Black),
];

pub fn palette_color(idx: usize) -> Color {
    PALETTE[idx % PALETTE.len()].1
}

pub fn palette_name(idx: usize) -> &'static str {
    PALETTE[idx % PALETTE.len()].0
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WallMode {
    Full,
    Open,
    Gaps,
}

impl WallMode {
    pub const ALL: [WallMode; 3] = [WallMode::Full, WallMode::Open, WallMode::Gaps];
    const LABELS: [&'static str; 3] = ["full_edge_wall", "no_edge_wall", "some_edge_wall"];
    const DISPLAY: [&'static str; 3] = ["solid", "none (wrap)", "gaps"];

    fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap()
    }

    pub fn label(self) -> &'static str {
        Self::LABELS[self.index()]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
    const LABELS: [&'static str; 4] = ["off", "few", "medium", "many"];

    fn index(self) -> usize {
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GrowthMode {
    Instant,
    AtTail,
}

impl GrowthMode {
    pub const ALL: [GrowthMode; 2] = [GrowthMode::Instant, GrowthMode::AtTail];
    const LABELS: [&'static str; 2] = ["instant", "at_tail"];
    const DISPLAY: [&'static str; 2] = ["instant", "when food reaches tail"];

    fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap()
    }

    pub fn label(self) -> &'static str {
        Self::DISPLAY[self.index()]
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

#[derive(Clone)]
pub struct GameConfig {
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
    pub icons: [usize; ICON_SLOTS],
    pub colors: [usize; COLOR_SLOTS],
}

impl Default for GameConfig {
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
            // food ●, super food ★, poison ✖
            icons: [0, 4, 11],
            // head, body, food, super food, poison, wall, background
            colors: [3, 2, 1, 5, 8, 13, 15],
        }
    }
}

#[derive(Clone, Copy)]
pub enum NumFmt {
    Plain,
    Millis,
    Secs,
    SecsOrOff,
    Plus,
}

impl NumFmt {
    fn render(self, v: u64) -> String {
        match self {
            NumFmt::Plain => v.to_string(),
            NumFmt::Millis => format!("{v} ms"),
            NumFmt::Secs => format!("{v} s"),
            NumFmt::SecsOrOff => {
                if v == 0 {
                    "off".into()
                } else {
                    format!("{v} s")
                }
            }
            NumFmt::Plus => format!("+{v}"),
        }
    }
}

pub enum Kind {
    Num {
        get: fn(&GameConfig) -> u64,
        set: fn(&mut GameConfig, u64),
        min: u64,
        max: u64,
        step: u64,
        fmt: NumFmt,
    },
    Choice {
        get: fn(&GameConfig) -> usize,
        set: fn(&mut GameConfig, usize),
        saved: &'static [&'static str],
        shown: &'static [&'static str],
    },
    Color(ColorSlot),
    Icon(IconSlot),
}

pub struct Setting {
    pub label: &'static str,
    pub key: &'static str,
    pub kind: Kind,
}

pub const SECTIONS: &[(&str, usize)] = &[
    ("Board", 0),
    ("Food", 5),
    ("Gameplay", 11),
    ("Icons", 15),
    ("Colors", 18),
];

pub const SETTINGS: &[Setting] = &[
    Setting {
        label: "board width",
        key: "width",
        kind: Kind::Num {
            get: |c| c.width as u64,
            set: |c, v| c.width = v as u16,
            min: 10,
            max: 80,
            step: 1,
            fmt: NumFmt::Plain,
        },
    },
    Setting {
        label: "board height",
        key: "height",
        kind: Kind::Num {
            get: |c| c.height as u64,
            set: |c, v| c.height = v as u16,
            min: 8,
            max: 40,
            step: 1,
            fmt: NumFmt::Plain,
        },
    },
    Setting {
        label: "tick speed",
        key: "tick_ms",
        kind: Kind::Num {
            get: |c| c.tick_ms,
            set: |c, v| c.tick_ms = v,
            min: 50,
            max: 1000,
            step: 25,
            fmt: NumFmt::Millis,
        },
    },
    Setting {
        label: "edge walls",
        key: "edge_walls",
        kind: Kind::Choice {
            get: |c| c.wall_mode.index(),
            set: |c, i| c.wall_mode = WallMode::ALL[i],
            saved: &WallMode::LABELS,
            shown: &WallMode::DISPLAY,
        },
    },
    Setting {
        label: "inner walls",
        key: "inner_walls",
        kind: Kind::Choice {
            get: |c| c.inner_walls.index(),
            set: |c, i| c.inner_walls = InnerWalls::ALL[i],
            saved: &InnerWalls::LABELS,
            shown: &InnerWalls::LABELS,
        },
    },
    Setting {
        label: "food spawns every",
        key: "food_interval_s",
        kind: Kind::Num {
            get: |c| c.food_interval_s,
            set: |c, v| c.food_interval_s = v,
            min: 1,
            max: 60,
            step: 1,
            fmt: NumFmt::Secs,
        },
    },
    Setting {
        label: "food lifetime",
        key: "food_ttl_s",
        kind: Kind::Num {
            get: |c| c.food_ttl_s,
            set: |c, v| c.food_ttl_s = v,
            min: 0,
            max: 120,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "super food spawns every",
        key: "super_interval_s",
        kind: Kind::Num {
            get: |c| c.super_interval_s,
            set: |c, v| c.super_interval_s = v,
            min: 0,
            max: 180,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "super food lifetime",
        key: "super_ttl_s",
        kind: Kind::Num {
            get: |c| c.super_ttl_s,
            set: |c, v| c.super_ttl_s = v,
            min: 0,
            max: 120,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "poison spawns every",
        key: "poison_interval_s",
        kind: Kind::Num {
            get: |c| c.poison_interval_s,
            set: |c, v| c.poison_interval_s = v,
            min: 0,
            max: 180,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "poison lifetime",
        key: "poison_ttl_s",
        kind: Kind::Num {
            get: |c| c.poison_ttl_s,
            set: |c, v| c.poison_ttl_s = v,
            min: 0,
            max: 120,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "growth mode",
        key: "growth",
        kind: Kind::Choice {
            get: |c| c.growth_mode.index(),
            set: |c, i| c.growth_mode = GrowthMode::ALL[i],
            saved: &GrowthMode::LABELS,
            shown: &GrowthMode::DISPLAY,
        },
    },
    Setting {
        label: "pass through self",
        key: "pass_through_self",
        kind: Kind::Choice {
            get: |c| c.pass_through_self as usize,
            set: |c, i| c.pass_through_self = i != 0,
            saved: &["false", "true"],
            shown: &["no", "yes"],
        },
    },
    Setting {
        label: "super food growth",
        key: "super_growth",
        kind: Kind::Num {
            get: |c| c.super_growth as u64,
            set: |c, v| c.super_growth = v as u32,
            min: 1,
            max: 9,
            step: 1,
            fmt: NumFmt::Plus,
        },
    },
    Setting {
        label: "super food speed boost",
        key: "boost_duration_s",
        kind: Kind::Num {
            get: |c| c.boost_duration_s,
            set: |c, v| c.boost_duration_s = v,
            min: 0,
            max: 60,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "food icon",
        key: "icon_food",
        kind: Kind::Icon(IconSlot::Food),
    },
    Setting {
        label: "super food icon",
        key: "icon_super",
        kind: Kind::Icon(IconSlot::SuperFood),
    },
    Setting {
        label: "poison icon",
        key: "icon_poison",
        kind: Kind::Icon(IconSlot::Poison),
    },
    Setting {
        label: "snake head",
        key: "color_head",
        kind: Kind::Color(ColorSlot::Head),
    },
    Setting {
        label: "snake body",
        key: "color_body",
        kind: Kind::Color(ColorSlot::Body),
    },
    Setting {
        label: "food",
        key: "color_food",
        kind: Kind::Color(ColorSlot::Food),
    },
    Setting {
        label: "super food",
        key: "color_super",
        kind: Kind::Color(ColorSlot::SuperFood),
    },
    Setting {
        label: "poison",
        key: "color_poison",
        kind: Kind::Color(ColorSlot::Poison),
    },
    Setting {
        label: "walls",
        key: "color_wall",
        kind: Kind::Color(ColorSlot::Wall),
    },
    Setting {
        label: "background",
        key: "color_bg",
        kind: Kind::Color(ColorSlot::Background),
    },
];

impl GameConfig {
    pub fn color(&self, slot: ColorSlot) -> Color {
        palette_color(self.colors[slot as usize])
    }

    pub fn icon(&self, slot: IconSlot) -> &'static str {
        glyph_char(self.icons[slot as usize])
    }

    pub fn value_str(&self, i: usize) -> String {
        match &SETTINGS[i].kind {
            Kind::Num { get, fmt, .. } => fmt.render(get(self)),
            Kind::Choice { get, shown, .. } => shown[get(self)].to_string(),
            Kind::Color(slot) => palette_name(self.colors[*slot as usize]).to_string(),
            Kind::Icon(slot) => glyph_name(self.icons[*slot as usize]).to_string(),
        }
    }

    pub fn swatch(&self, i: usize) -> Option<Color> {
        match SETTINGS[i].kind {
            Kind::Color(slot) => Some(self.color(slot)),
            _ => None,
        }
    }

    /// Glyph preview (glyph, its in-game color) for setting `i`, if it is an
    /// icon setting.
    pub fn icon_preview(&self, i: usize) -> Option<(&'static str, Color)> {
        match SETTINGS[i].kind {
            Kind::Icon(slot) => Some((self.icon(slot), self.color(slot.color_slot()))),
            _ => None,
        }
    }

    pub fn adjust(&mut self, i: usize, delta: i64) {
        let cycle = |cur: usize, len: usize| (cur as i64 + delta).rem_euclid(len as i64) as usize;
        match &SETTINGS[i].kind {
            Kind::Num {
                get,
                set,
                min,
                max,
                step,
                ..
            } => {
                let v = (get(self) as i64 + delta * *step as i64).clamp(*min as i64, *max as i64);
                set(self, v as u64);
            }
            Kind::Choice {
                get, set, saved, ..
            } => set(self, cycle(get(self), saved.len())),
            Kind::Color(slot) => {
                let slot = *slot as usize;
                self.colors[slot] = cycle(self.colors[slot], PALETTE.len());
            }
            Kind::Icon(slot) => {
                let slot = *slot as usize;
                self.icons[slot] = cycle(self.icons[slot], GLYPHS.len());
            }
        }
    }

    fn saved_value(&self, i: usize) -> String {
        match &SETTINGS[i].kind {
            Kind::Num { get, .. } => get(self).to_string(),
            Kind::Choice { get, saved, .. } => saved[get(self)].to_string(),
            Kind::Color(slot) => self.colors[*slot as usize].to_string(),
            Kind::Icon(slot) => self.icons[*slot as usize].to_string(),
        }
    }

    fn set_from_saved(&mut self, i: usize, value: &str) {
        match &SETTINGS[i].kind {
            Kind::Num { set, min, max, .. } => {
                if let Ok(n) = value.parse::<u64>() {
                    set(self, n.clamp(*min, *max));
                }
            }
            Kind::Choice { set, saved, .. } => {
                if let Some(idx) = saved.iter().position(|s| *s == value) {
                    set(self, idx);
                }
            }
            Kind::Color(slot) => {
                if let Ok(n) = value.parse::<usize>() {
                    self.colors[*slot as usize] = n % PALETTE.len();
                }
            }
            Kind::Icon(slot) => {
                if let Ok(n) = value.parse::<usize>() {
                    self.icons[*slot as usize] = n % GLYPHS.len();
                }
            }
        }
    }

    fn path() -> PathBuf {
        data_dir().join("config")
    }

    pub fn load() -> Self {
        let mut cfg = Self::default();
        let Ok(contents) = fs::read_to_string(Self::path()) else {
            return cfg;
        };
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if let Some(i) = SETTINGS.iter().position(|s| s.key == key.trim()) {
                cfg.set_from_saved(i, value.trim());
            }
        }
        cfg
    }

    pub fn save(&self) {
        let contents: String = SETTINGS
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}={}\n", s.key, self.saved_value(i)))
            .collect();
        let _ = fs::write(Self::path(), contents);
    }

    pub fn record_key(&self) -> String {
        format!(
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
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip() {
        let mut cfg = GameConfig::default();
        for i in 0..SETTINGS.len() {
            cfg.adjust(i, 1);
        }
        let mut restored = GameConfig::default();
        for (i, s) in SETTINGS.iter().enumerate() {
            let _ = s;
            restored.set_from_saved(i, &cfg.saved_value(i));
        }
        assert_eq!(cfg.record_key(), restored.record_key());
        assert_eq!(cfg.colors, restored.colors);
        assert_eq!(cfg.icons, restored.icons);
    }

    #[test]
    fn adjust_respects_bounds() {
        let mut cfg = GameConfig::default();
        for _ in 0..200 {
            cfg.adjust(0, -1);
        }
        assert_eq!(cfg.width, 10);
        for _ in 0..200 {
            cfg.adjust(0, 1);
        }
        assert_eq!(cfg.width, 80);
    }

    #[test]
    fn config_keys_are_unique() {
        for (i, a) in SETTINGS.iter().enumerate() {
            for b in &SETTINGS[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
    }
}
