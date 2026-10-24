use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use anyhow::Result;
use rand::RngExt;

use crate::board;
use crate::config::{GrowthMode, Rules};
use crate::geom::{Direction, Point, wrap_next};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FoodKind {
    Normal,
    Super,
    Poison,
}

impl FoodKind {
    pub const ALL: [FoodKind; 3] = [FoodKind::Normal, FoodKind::Super, FoodKind::Poison];
}

fn spawn_interval_s(rules: &Rules, kind: FoodKind) -> u64 {
    match kind {
        FoodKind::Normal => rules.food_interval_s,
        FoodKind::Super => rules.super_interval_s,
        FoodKind::Poison => rules.poison_interval_s,
    }
}

fn ttl_s(rules: &Rules, kind: FoodKind) -> u64 {
    match kind {
        FoodKind::Normal => rules.food_ttl_s,
        FoodKind::Super => rules.super_ttl_s,
        FoodKind::Poison => rules.poison_ttl_s,
    }
}

pub struct Food {
    pub pos: Point,
    pub kind: FoodKind,
    pub expires_at: Option<Duration>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameOverReason {
    HitWall,
    HitSelf,
    Poisoned,
    MaxLength,
}

pub struct Game {
    pub rules: Rules,
    pub width: u16,
    pub height: u16,
    walls: Vec<bool>,
    pub snake: VecDeque<Point>,
    pub dir: Direction,
    pub foods: Vec<Food>,
    growth: Vec<u32>,
    pub clock: Duration,
    tick_accum: Duration,
    next_spawn: [Option<Duration>; FoodKind::ALL.len()],
    boost_until: Duration,
    pub over: Option<GameOverReason>,
    free_cells: usize,
    pub started: bool,
}

impl Game {
    pub fn new(rules: Rules) -> Result<Self> {
        let (width, height) = (rules.width, rules.height);
        let mut rng = rand::rng();
        let walls = board::generate_walls(&rules, &mut rng);
        let free_cells = walls.iter().filter(|w| !**w).count();
        let (snake, dir) = board::place_snake(&walls, width, height, &mut rng)?;
        let next_spawn = FoodKind::ALL.map(|kind| {
            let interval = spawn_interval_s(&rules, kind);
            let first = if kind == FoodKind::Normal {
                1
            } else {
                interval
            };
            (interval > 0).then(|| Duration::from_secs(first))
        });
        Ok(Self {
            rules,
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

    /// The snake's length.
    #[expect(clippy::len_without_is_empty, reason = "a snake is never empty")]
    pub fn len(&self) -> usize {
        self.snake.len()
    }

    pub fn boost_active(&self) -> bool {
        self.clock < self.boost_until
    }

    pub fn tick_interval(&self) -> Duration {
        let base = Duration::from_millis(self.rules.tick_ms);
        if self.boost_active() { base / 2 } else { base }
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
        self.foods
            .retain(|f| f.expires_at.is_none_or(|e| e > clock));

        for (i, kind) in FoodKind::ALL.into_iter().enumerate() {
            let interval = spawn_interval_s(&self.rules, kind);
            while let Some(t) = self.next_spawn[i] {
                if clock < t {
                    break;
                }
                self.spawn_food(kind);
                self.next_spawn[i] = Some(t + Duration::from_secs(interval));
            }
        }

        self.tick_accum += dt;
        while self.over.is_none() && self.tick_accum >= self.tick_interval() {
            self.tick_accum -= self.tick_interval();
            self.step();
        }
    }

    fn step(&mut self) {
        let next = wrap_next(
            *self.snake.front().unwrap(),
            self.dir,
            self.width,
            self.height,
        );

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

        if !self.rules.pass_through_self && self.snake.contains(&next) {
            let tail = *self.snake.back().unwrap();
            let tail_vacates =
                !grow && next == tail && self.snake.iter().filter(|&&p| p == tail).count() == 1;
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
                    let n = self.rules.super_growth;
                    self.eat_growth(n);
                    if self.rules.boost_duration_s > 0 {
                        self.boost_until =
                            self.clock + Duration::from_secs(self.rules.boost_duration_s);
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
        match self.rules.growth_mode {
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
        let ttl = ttl_s(&self.rules, kind);
        let expires_at = (ttl > 0).then(|| self.clock + Duration::from_secs(ttl));
        self.foods.push(Food {
            pos,
            kind,
            expires_at,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InnerWalls, WallMode};

    #[test]
    fn snake_wraps_on_open_board() {
        let rules = Rules {
            wall_mode: WallMode::Open,
            inner_walls: InnerWalls::Off,
            ..Rules::default()
        };
        let mut game = Game::new(rules).unwrap();
        game.snake = VecDeque::from([Point { x: 0, y: 5 }]);
        game.dir = Direction::Left;
        game.turn(Direction::Left);
        assert_eq!(game.snake.front().unwrap().x, game.width - 1);
        assert!(game.over.is_none());
    }
}
