//! The settings menu as a table: what each setting is called, which section
//! it's shown in, how it's adjusted, and how it's saved as `key=value` text.

use crate::config::{
    COLOR_NAMES, ColorSlot, GLYPHS, GameConfig, GrowthMode, IconSlot, InnerWalls, MoveTiming,
    TICK_MS_MAX, TICK_MS_MIN, TICK_MS_STEP, WallMode, color_name, glyph_name,
};

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Board,
    Food,
    Gameplay,
    Icons,
    Colors,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Board => "Board",
            Section::Food => "Food",
            Section::Gameplay => "Gameplay",
            Section::Icons => "Icons",
            Section::Colors => "Colors",
        }
    }
}

pub struct Setting {
    pub label: &'static str,
    pub key: &'static str,
    /// Menu section. Settings of one section must be next to each other.
    pub section: Section,
    pub kind: Kind,
}

pub const SETTINGS: &[Setting] = &[
    Setting {
        label: "board width",
        key: "width",
        section: Section::Board,
        kind: Kind::Num {
            get: |c| c.rules.width as u64,
            set: |c, v| c.rules.width = v as u16,
            min: 10,
            max: 80,
            step: 1,
            fmt: NumFmt::Plain,
        },
    },
    Setting {
        label: "board height",
        key: "height",
        section: Section::Board,
        kind: Kind::Num {
            get: |c| c.rules.height as u64,
            set: |c, v| c.rules.height = v as u16,
            min: 8,
            max: 40,
            step: 1,
            fmt: NumFmt::Plain,
        },
    },
    Setting {
        label: "tick speed",
        key: "tick_ms",
        section: Section::Board,
        kind: Kind::Num {
            get: |c| c.rules.tick_ms,
            set: |c, v| c.rules.tick_ms = v,
            min: TICK_MS_MIN,
            max: TICK_MS_MAX,
            step: TICK_MS_STEP,
            fmt: NumFmt::Millis,
        },
    },
    Setting {
        label: "edge walls",
        key: "edge_walls",
        section: Section::Board,
        kind: Kind::Choice {
            get: |c| c.rules.wall_mode.index(),
            set: |c, i| c.rules.wall_mode = WallMode::ALL[i],
            saved: &WallMode::LABELS,
            shown: &WallMode::DISPLAY,
        },
    },
    Setting {
        label: "inner walls",
        key: "inner_walls",
        section: Section::Board,
        kind: Kind::Choice {
            get: |c| c.rules.inner_walls.index(),
            set: |c, i| c.rules.inner_walls = InnerWalls::ALL[i],
            saved: &InnerWalls::LABELS,
            shown: &InnerWalls::LABELS,
        },
    },
    Setting {
        label: "food spawns every",
        key: "food_interval_s",
        section: Section::Food,
        kind: Kind::Num {
            get: |c| c.rules.food_interval_s,
            set: |c, v| c.rules.food_interval_s = v,
            min: 1,
            max: 60,
            step: 1,
            fmt: NumFmt::Secs,
        },
    },
    Setting {
        label: "food lifetime",
        key: "food_ttl_s",
        section: Section::Food,
        kind: Kind::Num {
            get: |c| c.rules.food_ttl_s,
            set: |c, v| c.rules.food_ttl_s = v,
            min: 0,
            max: 120,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "super food spawns every",
        key: "super_interval_s",
        section: Section::Food,
        kind: Kind::Num {
            get: |c| c.rules.super_interval_s,
            set: |c, v| c.rules.super_interval_s = v,
            min: 0,
            max: 180,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "super food lifetime",
        key: "super_ttl_s",
        section: Section::Food,
        kind: Kind::Num {
            get: |c| c.rules.super_ttl_s,
            set: |c, v| c.rules.super_ttl_s = v,
            min: 0,
            max: 120,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "poison spawns every",
        key: "poison_interval_s",
        section: Section::Food,
        kind: Kind::Num {
            get: |c| c.rules.poison_interval_s,
            set: |c, v| c.rules.poison_interval_s = v,
            min: 0,
            max: 180,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "poison lifetime",
        key: "poison_ttl_s",
        section: Section::Food,
        kind: Kind::Num {
            get: |c| c.rules.poison_ttl_s,
            set: |c, v| c.rules.poison_ttl_s = v,
            min: 0,
            max: 120,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "move timing",
        key: "move_timing",
        section: Section::Gameplay,
        kind: Kind::Choice {
            get: |c| c.rules.move_timing.index(),
            set: |c, i| c.rules.move_timing = MoveTiming::ALL[i],
            saved: &MoveTiming::LABELS,
            shown: &MoveTiming::DISPLAY,
        },
    },
    Setting {
        label: "growth mode",
        key: "growth",
        section: Section::Gameplay,
        kind: Kind::Choice {
            get: |c| c.rules.growth_mode.index(),
            set: |c, i| c.rules.growth_mode = GrowthMode::ALL[i],
            saved: &GrowthMode::LABELS,
            shown: &GrowthMode::DISPLAY,
        },
    },
    Setting {
        label: "pass through self",
        key: "pass_through_self",
        section: Section::Gameplay,
        kind: Kind::Choice {
            get: |c| c.rules.pass_through_self as usize,
            set: |c, i| c.rules.pass_through_self = i != 0,
            saved: &["false", "true"],
            shown: &["no", "yes"],
        },
    },
    Setting {
        label: "super food growth",
        key: "super_growth",
        section: Section::Gameplay,
        kind: Kind::Num {
            get: |c| c.rules.super_growth as u64,
            set: |c, v| c.rules.super_growth = v as u32,
            min: 1,
            max: 9,
            step: 1,
            fmt: NumFmt::Plus,
        },
    },
    Setting {
        label: "super food speed boost",
        key: "boost_duration_s",
        section: Section::Gameplay,
        kind: Kind::Num {
            get: |c| c.rules.boost_duration_s,
            set: |c, v| c.rules.boost_duration_s = v,
            min: 0,
            max: 60,
            step: 1,
            fmt: NumFmt::SecsOrOff,
        },
    },
    Setting {
        label: "food icon",
        key: "icon_food",
        section: Section::Icons,
        kind: Kind::Icon(IconSlot::Food),
    },
    Setting {
        label: "super food icon",
        key: "icon_super",
        section: Section::Icons,
        kind: Kind::Icon(IconSlot::SuperFood),
    },
    Setting {
        label: "poison icon",
        key: "icon_poison",
        section: Section::Icons,
        kind: Kind::Icon(IconSlot::Poison),
    },
    Setting {
        label: "snake head",
        key: "color_head",
        section: Section::Colors,
        kind: Kind::Color(ColorSlot::Head),
    },
    Setting {
        label: "snake body",
        key: "color_body",
        section: Section::Colors,
        kind: Kind::Color(ColorSlot::Body),
    },
    Setting {
        label: "food",
        key: "color_food",
        section: Section::Colors,
        kind: Kind::Color(ColorSlot::Food),
    },
    Setting {
        label: "super food",
        key: "color_super",
        section: Section::Colors,
        kind: Kind::Color(ColorSlot::SuperFood),
    },
    Setting {
        label: "poison",
        key: "color_poison",
        section: Section::Colors,
        kind: Kind::Color(ColorSlot::Poison),
    },
    Setting {
        label: "walls",
        key: "color_wall",
        section: Section::Colors,
        kind: Kind::Color(ColorSlot::Wall),
    },
    Setting {
        label: "background",
        key: "color_bg",
        section: Section::Colors,
        kind: Kind::Color(ColorSlot::Background),
    },
];

impl GameConfig {
    pub fn value_str(&self, i: usize) -> String {
        match &SETTINGS[i].kind {
            Kind::Num { get, fmt, .. } => fmt.render(get(self)),
            Kind::Choice { get, shown, .. } => shown[get(self)].to_string(),
            Kind::Color(slot) => color_name(self.theme.colors[*slot as usize]).to_string(),
            Kind::Icon(slot) => glyph_name(self.theme.icons[*slot as usize]).to_string(),
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
                self.theme.colors[slot] = cycle(self.theme.colors[slot], COLOR_NAMES.len());
            }
            Kind::Icon(slot) => {
                let slot = *slot as usize;
                self.theme.icons[slot] = cycle(self.theme.icons[slot], GLYPHS.len());
            }
        }
    }

    fn saved_value(&self, i: usize) -> String {
        match &SETTINGS[i].kind {
            Kind::Num { get, .. } => get(self).to_string(),
            Kind::Choice { get, saved, .. } => saved[get(self)].to_string(),
            Kind::Color(slot) => self.theme.colors[*slot as usize].to_string(),
            Kind::Icon(slot) => self.theme.icons[*slot as usize].to_string(),
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
                    self.theme.colors[*slot as usize] = n % COLOR_NAMES.len();
                }
            }
            Kind::Icon(slot) => {
                if let Ok(n) = value.parse::<usize>() {
                    self.theme.icons[*slot as usize] = n % GLYPHS.len();
                }
            }
        }
    }

    /// The config as `key=value` lines, one per setting.
    pub fn to_saved_text(&self) -> String {
        SETTINGS
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}={}\n", s.key, self.saved_value(i)))
            .collect()
    }

    /// Parses [`Self::to_saved_text`] output. Unknown keys and bad values are
    /// skipped, leaving those settings at their defaults.
    pub fn from_saved_text(text: &str) -> Self {
        let mut cfg = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if let Some(i) = SETTINGS.iter().position(|s| s.key == key.trim()) {
                cfg.set_from_saved(i, value.trim());
            }
        }
        cfg
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
        assert_eq!(GameConfig::from_saved_text(&cfg.to_saved_text()), cfg);
    }

    #[test]
    fn reads_config_saved_by_older_versions() {
        let text = "width=42\ntick_ms=1000\nedge_walls=no_edge_wall\ngrowth=at_tail\n\
                    pass_through_self=true\ncolor_head=9\nicon_food=99\nnonsense\nunknown=1\n";
        let cfg = GameConfig::from_saved_text(text);
        assert_eq!(cfg.rules.width, 42);
        assert_eq!(cfg.rules.tick_ms, 1000);
        assert_eq!(cfg.rules.wall_mode, WallMode::Open);
        assert_eq!(cfg.rules.growth_mode, GrowthMode::AtTail);
        assert!(cfg.rules.pass_through_self);
        assert_eq!(cfg.theme.colors[ColorSlot::Head as usize], 9);
        assert_eq!(cfg.theme.icons[IconSlot::Food as usize], 99 % GLYPHS.len());
        assert_eq!(cfg.rules.height, GameConfig::default().rules.height);
    }

    #[test]
    fn adjust_respects_bounds() {
        let mut cfg = GameConfig::default();
        for _ in 0..200 {
            cfg.adjust(0, -1);
        }
        assert_eq!(cfg.rules.width, 10);
        for _ in 0..200 {
            cfg.adjust(0, 1);
        }
        assert_eq!(cfg.rules.width, 80);
    }

    #[test]
    fn config_keys_are_unique() {
        for (i, a) in SETTINGS.iter().enumerate() {
            for b in &SETTINGS[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
    }

    #[test]
    fn each_section_is_one_block() {
        let mut seen = Vec::new();
        for s in SETTINGS {
            if seen.last() != Some(&s.section) {
                assert!(!seen.contains(&s.section), "{:?} is split", s.section);
                seen.push(s.section);
            }
        }
    }
}
