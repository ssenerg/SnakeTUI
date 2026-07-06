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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WallMode {
    Full,
    Open,
    Gaps,
}

impl WallMode {
    pub fn label(self) -> &'static str {
        match self {
            WallMode::Full => "full_edge_wall",
            WallMode::Open => "no_edge_wall",
            WallMode::Gaps => "some_edge_wall",
        }
    }
    const ALL: [WallMode; 3] = [WallMode::Full, WallMode::Open, WallMode::Gaps];

    fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.label() == label)
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
    pub fn label(self) -> &'static str {
        match self {
            InnerWalls::Off => "off",
            InnerWalls::Few => "few",
            InnerWalls::Medium => "medium",
            InnerWalls::Many => "many",
        }
    }
        pub fn density_percent(self) -> usize {
        match self {
            InnerWalls::Off => 0,
            InnerWalls::Few => 6,
            InnerWalls::Medium => 12,
            InnerWalls::Many => 18,
        }
    }
    const ALL: [InnerWalls; 4] = [
        InnerWalls::Off,
        InnerWalls::Few,
        InnerWalls::Medium,
        InnerWalls::Many,
    ];

    fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.label() == label)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GrowthMode {
    Instant,
    AtTail,
}

impl GrowthMode {
    pub fn label(self) -> &'static str {
        match self {
            GrowthMode::Instant => "instant",
            GrowthMode::AtTail => "when food reaches tail",
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
    pub food_ttl_s: u64,     pub super_interval_s: u64,
    pub super_ttl_s: u64,
    pub poison_interval_s: u64,
    pub poison_ttl_s: u64,
    pub growth_mode: GrowthMode,
    pub pass_through_self: bool,
    pub super_growth: u32,
        pub boost_duration_s: u64,
        pub color_head: usize,
    pub color_body: usize,
    pub color_food: usize,
    pub color_super: usize,
    pub color_poison: usize,
    pub color_wall: usize,
    pub color_bg: usize,
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
            color_head: 3,               color_body: 2,               color_food: 1,               color_super: 5,              color_poison: 8,             color_wall: 13,              color_bg: 15,            }
    }
}

pub const CONFIG_ITEMS: usize = 22;

fn step_num(v: u64, delta: i64, min: u64, max: u64, step: u64) -> u64 {
    (v as i64 + delta * step as i64).clamp(min as i64, max as i64) as u64
}

fn secs_or_off(v: u64) -> String {
    if v == 0 { "off".into() } else { format!("{v} s") }
}

impl GameConfig {
    pub fn item_label(i: usize) -> &'static str {
        match i {
            0 => "board width",
            1 => "board height",
            2 => "tick speed",
            3 => "edge walls",
            4 => "inner walls",
            5 => "food spawns every",
            6 => "food lifetime",
            7 => "super food spawns every",
            8 => "super food lifetime",
            9 => "poison spawns every",
            10 => "poison lifetime",
            11 => "growth mode",
            12 => "pass through self",
            13 => "super food growth",
            14 => "super food speed boost",
            15 => "color: snake head",
            16 => "color: snake body",
            17 => "color: food",
            18 => "color: super food",
            19 => "color: poison",
            20 => "color: wall",
            21 => "color: background",
            _ => "",
        }
    }

    pub fn item_value(&self, i: usize) -> String {
        match i {
            0 => self.width.to_string(),
            1 => self.height.to_string(),
            2 => format!("{} ms", self.tick_ms),
            3 => self.wall_mode.label().into(),
            4 => self.inner_walls.label().into(),
            5 => format!("{} s", self.food_interval_s),
            6 => secs_or_off(self.food_ttl_s),
            7 => secs_or_off(self.super_interval_s),
            8 => secs_or_off(self.super_ttl_s),
            9 => secs_or_off(self.poison_interval_s),
            10 => secs_or_off(self.poison_ttl_s),
            11 => self.growth_mode.label().into(),
            12 => if self.pass_through_self { "yes" } else { "no" }.into(),
            13 => format!("+{}", self.super_growth),
            14 => secs_or_off(self.boost_duration_s),
            15 => palette_name(self.color_head).into(),
            16 => palette_name(self.color_body).into(),
            17 => palette_name(self.color_food).into(),
            18 => palette_name(self.color_super).into(),
            19 => palette_name(self.color_poison).into(),
            20 => palette_name(self.color_wall).into(),
            21 => palette_name(self.color_bg).into(),
            _ => String::new(),
        }
    }

        pub fn item_swatch(&self, i: usize) -> Option<Color> {
        let idx = match i {
            15 => self.color_head,
            16 => self.color_body,
            17 => self.color_food,
            18 => self.color_super,
            19 => self.color_poison,
            20 => self.color_wall,
            21 => self.color_bg,
            _ => return None,
        };
        Some(palette_color(idx))
    }

    pub fn adjust(&mut self, i: usize, delta: i64) {
        let cycle = |cur: usize, len: usize| (cur as i64 + delta).rem_euclid(len as i64) as usize;
        match i {
            0 => self.width = step_num(self.width as u64, delta, 10, 80, 1) as u16,
            1 => self.height = step_num(self.height as u64, delta, 8, 40, 1) as u16,
            2 => self.tick_ms = step_num(self.tick_ms, delta, 50, 1000, 25),
            3 => {
                let cur = WallMode::ALL.iter().position(|m| *m == self.wall_mode).unwrap();
                self.wall_mode = WallMode::ALL[cycle(cur, WallMode::ALL.len())];
            }
            4 => {
                let cur = InnerWalls::ALL.iter().position(|m| *m == self.inner_walls).unwrap();
                self.inner_walls = InnerWalls::ALL[cycle(cur, InnerWalls::ALL.len())];
            }
            5 => self.food_interval_s = step_num(self.food_interval_s, delta, 1, 60, 1),
            6 => self.food_ttl_s = step_num(self.food_ttl_s, delta, 0, 120, 1),
            7 => self.super_interval_s = step_num(self.super_interval_s, delta, 0, 180, 1),
            8 => self.super_ttl_s = step_num(self.super_ttl_s, delta, 0, 120, 1),
            9 => self.poison_interval_s = step_num(self.poison_interval_s, delta, 0, 180, 1),
            10 => self.poison_ttl_s = step_num(self.poison_ttl_s, delta, 0, 120, 1),
            11 => {
                self.growth_mode = match self.growth_mode {
                    GrowthMode::Instant => GrowthMode::AtTail,
                    GrowthMode::AtTail => GrowthMode::Instant,
                }
            }
            12 => self.pass_through_self = !self.pass_through_self,
            13 => self.super_growth = step_num(self.super_growth as u64, delta, 1, 9, 1) as u32,
            14 => self.boost_duration_s = step_num(self.boost_duration_s, delta, 0, 60, 1),
            15 => self.color_head = cycle(self.color_head, PALETTE.len()),
            16 => self.color_body = cycle(self.color_body, PALETTE.len()),
            17 => self.color_food = cycle(self.color_food, PALETTE.len()),
            18 => self.color_super = cycle(self.color_super, PALETTE.len()),
            19 => self.color_poison = cycle(self.color_poison, PALETTE.len()),
            20 => self.color_wall = cycle(self.color_wall, PALETTE.len()),
            21 => self.color_bg = cycle(self.color_bg, PALETTE.len()),
            _ => {}
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
            let (key, value) = (key.trim(), value.trim());
            let num = value.parse::<u64>();
            match key {
                "width" => {
                    if let Ok(n) = num {
                        cfg.width = n.clamp(10, 80) as u16;
                    }
                }
                "height" => {
                    if let Ok(n) = num {
                        cfg.height = n.clamp(8, 40) as u16;
                    }
                }
                "tick_ms" => {
                    if let Ok(n) = num {
                        cfg.tick_ms = n.clamp(50, 1000);
                    }
                }
                "edge_walls" => {
                    if let Some(m) = WallMode::from_label(value) {
                        cfg.wall_mode = m;
                    }
                }
                "inner_walls" => {
                    if let Some(m) = InnerWalls::from_label(value) {
                        cfg.inner_walls = m;
                    }
                }
                "food_interval_s" => {
                    if let Ok(n) = num {
                        cfg.food_interval_s = n.clamp(1, 60);
                    }
                }
                "food_ttl_s" => {
                    if let Ok(n) = num {
                        cfg.food_ttl_s = n.min(120);
                    }
                }
                "super_interval_s" => {
                    if let Ok(n) = num {
                        cfg.super_interval_s = n.min(180);
                    }
                }
                "super_ttl_s" => {
                    if let Ok(n) = num {
                        cfg.super_ttl_s = n.min(120);
                    }
                }
                "poison_interval_s" => {
                    if let Ok(n) = num {
                        cfg.poison_interval_s = n.min(180);
                    }
                }
                "poison_ttl_s" => {
                    if let Ok(n) = num {
                        cfg.poison_ttl_s = n.min(120);
                    }
                }
                "growth" => {
                    cfg.growth_mode = match value {
                        "instant" => GrowthMode::Instant,
                        "at_tail" => GrowthMode::AtTail,
                        _ => cfg.growth_mode,
                    }
                }
                "pass_through_self" => cfg.pass_through_self = value == "true",
                "super_growth" => {
                    if let Ok(n) = num {
                        cfg.super_growth = n.clamp(1, 9) as u32;
                    }
                }
                "boost_duration_s" => {
                    if let Ok(n) = num {
                        cfg.boost_duration_s = n.min(60);
                    }
                }
                "color_head" => {
                    if let Ok(n) = num {
                        cfg.color_head = n as usize % PALETTE.len();
                    }
                }
                "color_body" => {
                    if let Ok(n) = num {
                        cfg.color_body = n as usize % PALETTE.len();
                    }
                }
                "color_food" => {
                    if let Ok(n) = num {
                        cfg.color_food = n as usize % PALETTE.len();
                    }
                }
                "color_super" => {
                    if let Ok(n) = num {
                        cfg.color_super = n as usize % PALETTE.len();
                    }
                }
                "color_poison" => {
                    if let Ok(n) = num {
                        cfg.color_poison = n as usize % PALETTE.len();
                    }
                }
                "color_wall" => {
                    if let Ok(n) = num {
                        cfg.color_wall = n as usize % PALETTE.len();
                    }
                }
                "color_bg" => {
                    if let Ok(n) = num {
                        cfg.color_bg = n as usize % PALETTE.len();
                    }
                }
                _ => {}
            }
        }
        cfg
    }

    pub fn save(&self) {
        let growth = match self.growth_mode {
            GrowthMode::Instant => "instant",
            GrowthMode::AtTail => "at_tail",
        };
        let contents = format!(
            "width={}\nheight={}\ntick_ms={}\nedge_walls={}\ninner_walls={}\n\
             food_interval_s={}\nfood_ttl_s={}\nsuper_interval_s={}\nsuper_ttl_s={}\n\
             poison_interval_s={}\npoison_ttl_s={}\ngrowth={}\npass_through_self={}\n\
             super_growth={}\nboost_duration_s={}\ncolor_head={}\ncolor_body={}\n\
             color_food={}\ncolor_super={}\ncolor_poison={}\ncolor_wall={}\ncolor_bg={}\n",
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
            growth,
            self.pass_through_self,
            self.super_growth,
            self.boost_duration_s,
            self.color_head,
            self.color_body,
            self.color_food,
            self.color_super,
            self.color_poison,
            self.color_wall,
            self.color_bg,
        );
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
