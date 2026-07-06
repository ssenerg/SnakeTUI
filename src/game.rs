use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use anyhow::{Result, anyhow};
use rand::RngExt;

use crate::config::{GameConfig, GrowthMode, WallMode};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub fn opposite(self) -> Self {
        match self {
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FoodKind {
    Normal,
    Super,
    Poison,
}

pub struct Food {
    pub pos: Point,
    pub kind: FoodKind,
    pub expires_at: Option<Duration>,
}

#[derive(Clone, Copy)]
pub enum GameOverReason {
    HitWall,
    HitSelf,
    Poisoned,
    MaxLength,
}

pub struct Game {
    pub cfg: GameConfig,
    pub width: u16,
    pub height: u16,
    walls: Vec<bool>,
    pub snake: VecDeque<Point>,
    pub dir: Direction,
    pub foods: Vec<Food>,
    growth: Vec<u32>,
    pub clock: Duration,
    tick_accum: Duration,
    next_spawn: [Option<Duration>; 3],
    boost_until: Duration,
    pub over: Option<GameOverReason>,
    free_cells: usize,
    pub started: bool,
}

impl Game {
    pub fn new(cfg: GameConfig) -> Result<Self> {
        let (width, height) = (cfg.width, cfg.height);
        let walls = generate_walls(&cfg);
        let free_cells = walls.iter().filter(|w| !**w).count();
        let (snake, dir) = place_snake(&walls, width, height)?;
        let next_spawn = [
            (cfg.food_interval_s > 0).then(|| Duration::from_secs(1)),
            (cfg.super_interval_s > 0).then(|| Duration::from_secs(cfg.super_interval_s)),
            (cfg.poison_interval_s > 0).then(|| Duration::from_secs(cfg.poison_interval_s)),
        ];
        Ok(Self {
            cfg,
            width,
            height,
            walls,
            snake,
            dir,
            foods: Vec::new(),
            growth: Vec::new(),
            clock: Duration::ZERO,
            tick_accum: Duration::ZERO,
            next_spawn,
            boost_until: Duration::ZERO,
            over: None,
            free_cells,
            started: false,
        })
    }

    fn idx(&self, p: Point) -> usize {
        p.y as usize * self.width as usize + p.x as usize
    }

    pub fn wall_at(&self, x: u16, y: u16) -> bool {
        self.walls[y as usize * self.width as usize + x as usize]
    }

    pub fn len(&self) -> usize {
        self.snake.len()
    }

    pub fn boost_active(&self) -> bool {
        self.clock < self.boost_until
    }

    pub fn tick_interval(&self) -> Duration {
        let base = Duration::from_millis(self.cfg.tick_ms);
        if self.boost_active() { base / 2 } else { base }
    }

    fn wrap_next(&self, p: Point, dir: Direction) -> Point {
        let (w, h) = (self.width, self.height);
        match dir {
            Direction::Up => Point { x: p.x, y: (p.y + h - 1) % h },
            Direction::Down => Point { x: p.x, y: (p.y + 1) % h },
            Direction::Left => Point { x: (p.x + w - 1) % w, y: p.y },
            Direction::Right => Point { x: (p.x + 1) % w, y: p.y },
        }
    }

    pub fn turn(&mut self, dir: Direction) {
        if self.over.is_some() {
            return;
        }
        if self.snake.len() > 1 && dir == self.dir.opposite() {
            return;
        }
        self.started = true;
        self.dir = dir;
        self.tick_accum = Duration::ZERO;
        self.step();
    }

    pub fn advance(&mut self, dt: Duration) {
        if self.over.is_some() || !self.started {
            return;
        }
        let dt = dt.min(Duration::from_millis(250));
        self.clock += dt;

        let clock = self.clock;
        self.foods.retain(|f| f.expires_at.is_none_or(|e| e > clock));

        for k in 0..3 {
            let (kind, interval) = match k {
                0 => (FoodKind::Normal, self.cfg.food_interval_s),
                1 => (FoodKind::Super, self.cfg.super_interval_s),
                _ => (FoodKind::Poison, self.cfg.poison_interval_s),
            };
            while let Some(t) = self.next_spawn[k] {
                if clock < t {
                    break;
                }
                self.spawn_food(kind);
                self.next_spawn[k] = Some(t + Duration::from_secs(interval));
            }
        }

        self.tick_accum += dt;
        while self.over.is_none() && self.tick_accum >= self.tick_interval() {
            self.tick_accum -= self.tick_interval();
            self.step();
        }
    }

    fn step(&mut self) {
        let next = self.wrap_next(*self.snake.front().unwrap(), self.dir);

        if self.walls[self.idx(next)] {
            self.over = Some(GameOverReason::HitWall);
            return;
        }

        for t in self.growth.iter_mut() {
            if *t > 0 {
                *t -= 1;
            }
        }
        let mut grow = false;
        if let Some(i) = self.growth.iter().position(|&t| t == 0) {
            self.growth.remove(i);
            grow = true;
        }

        if !self.cfg.pass_through_self && self.snake.contains(&next) {
            let tail = *self.snake.back().unwrap();
            let tail_vacates = !grow
                && next == tail
                && self.snake.iter().filter(|&&p| p == tail).count() == 1;
            if !tail_vacates {
                self.over = Some(GameOverReason::HitSelf);
                return;
            }
        }

        self.snake.push_front(next);
        if !grow {
            self.snake.pop_back();
        }

        if let Some(i) = self.foods.iter().position(|f| f.pos == next) {
            let food = self.foods.remove(i);
            match food.kind {
                FoodKind::Normal => self.eat_growth(1),
                FoodKind::Super => {
                    let n = self.cfg.super_growth;
                    self.eat_growth(n);
                    if self.cfg.boost_duration_s > 0 {
                        self.boost_until =
                            self.clock + Duration::from_secs(self.cfg.boost_duration_s);
                    }
                }
                FoodKind::Poison => {
                    self.over = Some(GameOverReason::Poisoned);
                    return;
                }
            }
        }

        if self.snake.len() >= self.free_cells {
            let distinct: HashSet<Point> = self.snake.iter().copied().collect();
            if distinct.len() >= self.free_cells {
                self.over = Some(GameOverReason::MaxLength);
            }
        }
    }

    fn eat_growth(&mut self, n: u32) {
        match self.cfg.growth_mode {
            GrowthMode::Instant => {
                let tail = *self.snake.back().unwrap();
                for _ in 0..n {
                    self.snake.push_back(tail);
                }
            }
            GrowthMode::AtTail => {
                let len = self.snake.len() as u32;
                for _ in 0..n {
                    self.growth.push(len);
                }
            }
        }
    }

    fn spawn_food(&mut self, kind: FoodKind) {
        let occupied: HashSet<Point> = self
            .snake
            .iter()
            .copied()
            .chain(self.foods.iter().map(|f| f.pos))
            .collect();
        let mut cells = Vec::new();
        for y in 0..self.height {
            for x in 0..self.width {
                let p = Point { x, y };
                if !self.walls[self.idx(p)] && !occupied.contains(&p) {
                    cells.push(p);
                }
            }
        }
        if cells.is_empty() {
            return;
        }
        let mut rng = rand::rng();
        let pos = cells[rng.random_range(0..cells.len())];
        let ttl = match kind {
            FoodKind::Normal => self.cfg.food_ttl_s,
            FoodKind::Super => self.cfg.super_ttl_s,
            FoodKind::Poison => self.cfg.poison_ttl_s,
        };
        let expires_at = (ttl > 0).then(|| self.clock + Duration::from_secs(ttl));
        self.foods.push(Food { pos, kind, expires_at });
    }
}

fn generate_walls(cfg: &GameConfig) -> Vec<bool> {
    let w = cfg.width as usize;
    let h = cfg.height as usize;
    let mut walls = vec![false; w * h];
    let mut rng = rand::rng();

    match cfg.wall_mode {
        WallMode::Full => {
            for x in 0..w {
                walls[x] = true;
                walls[(h - 1) * w + x] = true;
            }
            for y in 0..h {
                walls[y * w] = true;
                walls[y * w + w - 1] = true;
            }
        }
        WallMode::Open => {}
        WallMode::Gaps => {
            for &(x, y) in &[(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
                walls[y * w + x] = true;
            }
            for x in 1..w - 1 {
                let is_wall = rng.random_range(0..100) < 65;
                walls[x] = is_wall;
                walls[(h - 1) * w + x] = is_wall;
            }
            for y in 1..h - 1 {
                let is_wall = rng.random_range(0..100) < 65;
                walls[y * w] = is_wall;
                walls[y * w + w - 1] = is_wall;
            }
        }
    }

    let density = cfg.inner_walls.density_percent();
    if density > 0 && w > 2 && h > 2 {
        let target = (w - 2) * (h - 2) * density / 100;
        let mut placed = 0;
        let mut attempts = 0;
        while placed < target && attempts < target * 50 + 100 {
            attempts += 1;
            let x = rng.random_range(1..w - 1);
            let y = rng.random_range(1..h - 1);
            let i = y * w + x;
            if walls[i] {
                continue;
            }
            walls[i] = true;
            if all_free_connected(&walls, w, h) && no_dead_ends(&walls, w, h) {
                placed += 1;
            } else {
                walls[i] = false;
            }
        }
    }

    walls
}

fn no_dead_ends(walls: &[bool], w: usize, h: usize) -> bool {
    for y in 0..h {
        for x in 0..w {
            if walls[y * w + x] {
                continue;
            }
            let exits = [
                ((x + 1) % w, y),
                ((x + w - 1) % w, y),
                (x, (y + 1) % h),
                (x, (y + h - 1) % h),
            ]
            .iter()
            .filter(|&&(nx, ny)| !walls[ny * w + nx])
            .count();
            if exits < 2 {
                return false;
            }
        }
    }
    true
}

fn all_free_connected(walls: &[bool], w: usize, h: usize) -> bool {
    let total_free = walls.iter().filter(|x| !**x).count();
    let Some(start) = walls.iter().position(|x| !*x) else {
        return false;
    };
    let mut seen = vec![false; w * h];
    seen[start] = true;
    let mut count = 1;
    let mut queue = VecDeque::from([start]);
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        for (nx, ny) in [
            ((x + 1) % w, y),
            ((x + w - 1) % w, y),
            (x, (y + 1) % h),
            (x, (y + h - 1) % h),
        ] {
            let ni = ny * w + nx;
            if !seen[ni] && !walls[ni] {
                seen[ni] = true;
                count += 1;
                queue.push_back(ni);
            }
        }
    }
    count == total_free
}

fn place_snake(walls: &[bool], w: u16, h: u16) -> Result<(VecDeque<Point>, Direction)> {
    let wu = w as usize;
    let free: Vec<Point> = (0..h)
        .flat_map(|y| (0..w).map(move |x| Point { x, y }))
        .filter(|p| !walls[p.y as usize * wu + p.x as usize])
        .collect();
    if free.is_empty() {
        return Err(anyhow!("board has no free cells"));
    }
    let mut rng = rand::rng();

    for target_len in [3usize, 2, 1] {
        for _ in 0..300 {
            let mut body = vec![free[rng.random_range(0..free.len())]];
            while body.len() < target_len {
                let cur = *body.last().unwrap();
                let mut cands = Vec::new();
                if cur.x > 0 {
                    cands.push(Point { x: cur.x - 1, y: cur.y });
                }
                if cur.x + 1 < w {
                    cands.push(Point { x: cur.x + 1, y: cur.y });
                }
                if cur.y > 0 {
                    cands.push(Point { x: cur.x, y: cur.y - 1 });
                }
                if cur.y + 1 < h {
                    cands.push(Point { x: cur.x, y: cur.y + 1 });
                }
                cands.retain(|p| !walls[p.y as usize * wu + p.x as usize] && !body.contains(p));
                if cands.is_empty() {
                    break;
                }
                body.push(cands[rng.random_range(0..cands.len())]);
            }
            if body.len() == target_len {
                let dir = if target_len >= 2 {
                    let (head, neck) = (body[0], body[1]);
                    if head.x > neck.x {
                        Direction::Right
                    } else if head.x < neck.x {
                        Direction::Left
                    } else if head.y > neck.y {
                        Direction::Down
                    } else {
                        Direction::Up
                    }
                } else {
                    match rng.random_range(0..4) {
                        0 => Direction::Up,
                        1 => Direction::Down,
                        2 => Direction::Left,
                        _ => Direction::Right,
                    }
                };
                return Ok((VecDeque::from(body), dir));
            }
        }
    }
    Err(anyhow!("could not place the snake on the board"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::InnerWalls;

    #[test]
    fn gap_walls_are_mirrored_and_connected() {
        let mut cfg = GameConfig::default();
        cfg.wall_mode = WallMode::Gaps;
        cfg.inner_walls = InnerWalls::Many;
        for _ in 0..20 {
            let (w, h) = (cfg.width as usize, cfg.height as usize);
            let walls = generate_walls(&cfg);
            for x in 0..w {
                assert_eq!(walls[x], walls[(h - 1) * w + x], "top/bottom mirror at x={x}");
            }
            for y in 0..h {
                assert_eq!(walls[y * w], walls[y * w + w - 1], "left/right mirror at y={y}");
            }
            assert!(all_free_connected(&walls, w, h), "free area must be connected");
        }
    }

    #[test]
    fn full_walls_keep_interior_connected() {
        let mut cfg = GameConfig::default();
        cfg.wall_mode = WallMode::Full;
        cfg.inner_walls = InnerWalls::Many;
        for _ in 0..20 {
            let (w, h) = (cfg.width as usize, cfg.height as usize);
            let walls = generate_walls(&cfg);
            assert!(all_free_connected(&walls, w, h));
        }
    }

    #[test]
    fn inner_walls_never_create_dead_ends() {
        for wall_mode in [WallMode::Full, WallMode::Open, WallMode::Gaps] {
            let mut cfg = GameConfig::default();
            cfg.wall_mode = wall_mode;
            cfg.inner_walls = InnerWalls::Many;
            for _ in 0..20 {
                let (w, h) = (cfg.width as usize, cfg.height as usize);
                let walls = generate_walls(&cfg);
                assert!(no_dead_ends(&walls, w, h), "free cell with < 2 exits");
            }
        }
    }

    #[test]
    fn snake_wraps_on_open_board() {
        let mut cfg = GameConfig::default();
        cfg.wall_mode = WallMode::Open;
        cfg.inner_walls = InnerWalls::Off;
        let mut game = Game::new(cfg).unwrap();
        game.snake = VecDeque::from([Point { x: 0, y: 5 }]);
        game.dir = Direction::Left;
        game.turn(Direction::Left);
        assert_eq!(game.snake.front().unwrap().x, game.width - 1);
        assert!(game.over.is_none());
    }
}
