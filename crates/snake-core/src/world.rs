//! The board and everything on it: walls, food, and any number of snakes that
//! all move one cell at the same moment.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rand::rngs::Xoshiro256PlusPlus;
use rand::{RngExt, SeedableRng};

use crate::board;
use crate::config::Rules;
use crate::geom::{Direction, Point, wrap_next};
use crate::snake::{DeathCause, Snake};
use crate::spawn;
use crate::view::{MatchView, SnakeView};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Food {
    pub pos: Point,
    pub kind: FoodKind,
    pub expires_at: Option<Duration>,
}

/// What happened during one [`World::step`], in snake order.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct StepOutcome {
    pub deaths: Vec<(u8, DeathCause)>,
    pub eaten: Vec<(u8, FoodKind)>,
}

/// One living snake's move during a step.
struct Move {
    snake: usize,
    next: Point,
    /// How many of its current cells stay part of the body.
    keep: usize,
    death: Option<DeathCause>,
}

pub struct World {
    pub rules: Rules,
    pub width: u16,
    pub height: u16,
    walls: Arc<[bool]>,
    pub snakes: Vec<Snake>,
    pub foods: Vec<Food>,
    /// Game time. It only moves through [`Self::advance_clock`], so a world
    /// driven by fixed ticks replays exactly from its seed and inputs.
    pub clock: Duration,
    next_spawn: [Option<Duration>; FoodKind::ALL.len()],
    free_cells: usize,
    /// Whether dead snakes turn into food. Otherwise they stay where they
    /// died, as the single-player game-over screen shows.
    pub corpses_to_food: bool,
    rng: Xoshiro256PlusPlus,
    /// Scratch space for [`Self::step`]: snake segments per cell.
    occupancy: Vec<u16>,
}

impl World {
    /// A board with one snake placed at random, for single player.
    pub fn solo(rules: Rules, seed: u64) -> Result<Self> {
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed);
        let walls = board::generate_walls(&rules, &mut rng);
        let (body, dir) = board::place_snake(&walls, rules.width, rules.height, &mut rng)?;
        Ok(Self::new(rules, walls, vec![Snake::new(0, body, dir)], rng))
    }

    /// A board with `players` snakes spread apart, for multiplayer. Snakes
    /// that die turn into food.
    pub fn with_players(rules: Rules, seed: u64, players: usize) -> Result<Self> {
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed);
        let walls = board::generate_walls(&rules, &mut rng);
        let snakes = spawn::spawn_points(&walls, rules.width, rules.height, players, &mut rng)?
            .into_iter()
            .enumerate()
            .map(|(i, (body, dir))| Snake::new(i as u8, body, dir))
            .collect();
        let mut world = Self::new(rules, walls, snakes, rng);
        world.corpses_to_food = true;
        Ok(world)
    }

    fn new(rules: Rules, walls: Vec<bool>, snakes: Vec<Snake>, rng: Xoshiro256PlusPlus) -> Self {
        let free_cells = walls.iter().filter(|w| !**w).count();
        let next_spawn = FoodKind::ALL.map(|kind| {
            let interval = spawn_interval_s(&rules, kind);
            let first = if kind == FoodKind::Normal {
                1
            } else {
                interval
            };
            (interval > 0).then(|| Duration::from_secs(first))
        });
        Self {
            width: rules.width,
            height: rules.height,
            occupancy: vec![0; walls.len()],
            walls: walls.into(),
            rules,
            snakes,
            foods: Vec::new(),
            clock: Duration::ZERO,
            next_spawn,
            free_cells,
            corpses_to_food: false,
            rng,
        }
    }

    fn idx(&self, p: Point) -> usize {
        p.y as usize * self.width as usize + p.x as usize
    }

    pub fn wall_at(&self, x: u16, y: u16) -> bool {
        self.walls[self.idx(Point { x, y })]
    }

    /// How many cells aren't walls.
    pub fn free_cells(&self) -> usize {
        self.free_cells
    }

    /// Queues a turn for snake `id`'s upcoming ticks. Returns whether it was
    /// kept; keys for dead or unknown snakes are dropped.
    pub fn steer(&mut self, id: u8, dir: Direction) -> bool {
        let (w, h) = (self.width, self.height);
        self.snakes
            .iter_mut()
            .find(|s| s.id == id)
            .is_some_and(|s| s.enqueue(dir, w, h))
    }

    /// Moves game time forward: food that has run out disappears, and new
    /// food appears on schedule.
    pub fn advance_clock(&mut self, dt: Duration) {
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
    }

    /// Moves every living snake one cell, all at once.
    ///
    /// Each snake first takes its next queued turn. Then, judged against
    /// everyone's positions after the move, a snake dies when its head:
    /// - hits a wall;
    /// - lands on another snake's new head or body, or swaps places with
    ///   another head (so a head-on crash kills both);
    /// - lands on its own new body, unless pass-through-self is on;
    /// - eats poison.
    ///
    /// A tail that moves away this step frees its cell; a growing one doesn't.
    /// Survivors and poisoned snakes move; the rest die where they are. Last,
    /// surviving heads eat what they landed on. The order of `snakes` never
    /// changes the outcome.
    pub fn step(&mut self) -> StepOutcome {
        let (w, h) = (self.width, self.height);
        let wu = w as usize;
        let at = |p: Point| p.y as usize * wu + p.x as usize;
        let mut out = StepOutcome::default();

        let mut moves = Vec::new();
        for (i, snake) in self.snakes.iter_mut().enumerate() {
            if !snake.alive() {
                continue;
            }
            snake.take_turn(w, h);
            let next = wrap_next(snake.head(), snake.dir, w, h);
            let keep = snake.len() - usize::from(!snake.tick_growth());
            moves.push(Move {
                snake: i,
                next,
                keep,
                death: None,
            });
        }

        // Segments per cell once everyone has moved.
        self.occupancy.fill(0);
        for m in &moves {
            self.occupancy[at(m.next)] += 1;
            for &p in self.snakes[m.snake].body.iter().take(m.keep) {
                self.occupancy[at(p)] += 1;
            }
        }

        for k in 0..moves.len() {
            let m = &moves[k];
            let snake = &self.snakes[m.snake];
            let own = snake
                .body
                .iter()
                .take(m.keep)
                .filter(|&&p| p == m.next)
                .count();
            let others = usize::from(self.occupancy[at(m.next)]) - 1 - own;
            // One-cell snakes leave nothing behind to collide with, so check
            // swaps explicitly.
            let swapped = moves.iter().any(|o| {
                o.snake != m.snake
                    && o.next == snake.head()
                    && m.next == self.snakes[o.snake].head()
            });
            let poison = self
                .foods
                .iter()
                .any(|f| f.pos == m.next && f.kind == FoodKind::Poison);
            moves[k].death = if self.walls[at(m.next)] {
                Some(DeathCause::Wall)
            } else if others > 0 || swapped {
                Some(DeathCause::OtherSnake)
            } else if own > 0 && !self.rules.pass_through_self {
                Some(DeathCause::OwnBody)
            } else if poison {
                Some(DeathCause::Poison)
            } else {
                None
            };
        }

        for m in &moves {
            let snake = &mut self.snakes[m.snake];
            if matches!(m.death, None | Some(DeathCause::Poison)) {
                snake.body.push_front(m.next);
                snake.body.truncate(m.keep + 1);
            }
            if let Some(cause) = m.death {
                snake.death = Some(cause);
                out.deaths.push((snake.id, cause));
            }
        }

        for m in &moves {
            let Some(i) = self.foods.iter().position(|f| f.pos == m.next) else {
                continue;
            };
            match m.death {
                None => {
                    let food = self.foods.remove(i);
                    let n = match food.kind {
                        FoodKind::Super => self.rules.super_growth,
                        _ => 1,
                    };
                    let snake = &mut self.snakes[m.snake];
                    snake.grow(n, self.rules.growth_mode);
                    out.eaten.push((snake.id, food.kind));
                }
                Some(DeathCause::Poison) => {
                    self.foods.remove(i);
                }
                Some(_) => {}
            }
        }

        if self.corpses_to_food {
            for m in &moves {
                if m.death.is_some() {
                    self.turn_into_food(m.snake);
                }
            }
        }
        out
    }

    /// Replaces a dead snake's body with food, except where something else is.
    fn turn_into_food(&mut self, snake: usize) {
        let body = std::mem::take(&mut self.snakes[snake].body);
        for p in body {
            let taken = self.walls[self.idx(p)]
                || self.foods.iter().any(|f| f.pos == p)
                || self.snakes.iter().any(|s| s.alive() && s.body.contains(&p));
            if !taken {
                self.place_food(p, FoodKind::Normal);
            }
        }
    }

    fn spawn_food(&mut self, kind: FoodKind) {
        let mut taken: Vec<bool> = self.walls.to_vec();
        for p in self.snakes.iter().flat_map(|s| &s.body) {
            taken[self.idx(*p)] = true;
        }
        for f in &self.foods {
            taken[self.idx(f.pos)] = true;
        }
        let w = self.width as usize;
        let cells: Vec<Point> = (0..taken.len())
            .filter(|&i| !taken[i])
            .map(|i| Point {
                x: (i % w) as u16,
                y: (i / w) as u16,
            })
            .collect();
        if cells.is_empty() {
            return;
        }
        let pos = cells[self.rng.random_range(0..cells.len())];
        self.place_food(pos, kind);
    }

    fn place_food(&mut self, pos: Point, kind: FoodKind) {
        let ttl = ttl_s(&self.rules, kind);
        let expires_at = (ttl > 0).then(|| self.clock + Duration::from_secs(ttl));
        self.foods.push(Food {
            pos,
            kind,
            expires_at,
        });
    }

    pub fn view(&self) -> MatchView {
        MatchView {
            width: self.width,
            height: self.height,
            walls: Arc::clone(&self.walls),
            snakes: self
                .snakes
                .iter()
                .map(|s| SnakeView {
                    id: s.id,
                    body: s.body.iter().copied().collect(),
                    alive: s.alive(),
                })
                .collect(),
            foods: self.foods.iter().map(|f| (f.pos, f.kind)).collect(),
            clock: self.clock,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};

    use proptest::prelude::*;

    use super::*;
    use crate::config::{GrowthMode, InnerWalls, WallMode};
    use crate::geom::Direction::{self, Down, Left, Right, Up};

    fn rules(wall_mode: WallMode) -> Rules {
        Rules {
            wall_mode,
            inner_walls: InnerWalls::Off,
            ..Rules::default()
        }
    }

    /// A world with no food scheduled and the given snakes, ids in order.
    fn world(rules: Rules, snakes: &[(&[(u16, u16)], Direction)]) -> World {
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(0);
        let walls = board::generate_walls(&rules, &mut rng);
        let snakes = snakes
            .iter()
            .enumerate()
            .map(|(i, (cells, dir))| {
                let body: VecDeque<Point> = cells.iter().map(|&(x, y)| Point { x, y }).collect();
                Snake::new(i as u8, body, *dir)
            })
            .collect();
        let mut world = World::new(rules, walls, snakes, rng);
        world.next_spawn = [None; 3];
        world
    }

    fn body(world: &World, id: usize) -> Vec<(u16, u16)> {
        world.snakes[id].body.iter().map(|p| (p.x, p.y)).collect()
    }

    fn deaths(world: &mut World) -> Vec<(u8, DeathCause)> {
        world.step().deaths
    }

    use DeathCause::{OtherSnake, OwnBody, Poison, Wall};

    #[test]
    fn head_on_into_the_same_cell_kills_both() {
        let mut w = world(
            rules(WallMode::Open),
            &[
                (&[(5, 5), (4, 5), (3, 5)], Right),
                (&[(7, 5), (8, 5), (9, 5)], Left),
            ],
        );
        assert_eq!(deaths(&mut w), [(0, OtherSnake), (1, OtherSnake)]);
        assert_eq!(
            body(&w, 0),
            [(5, 5), (4, 5), (3, 5)],
            "dead snakes don't move"
        );
    }

    #[test]
    fn swapping_heads_kills_both_at_any_length() {
        let mut short = world(
            rules(WallMode::Open),
            &[(&[(5, 5)], Right), (&[(6, 5)], Left)],
        );
        assert_eq!(deaths(&mut short), [(0, OtherSnake), (1, OtherSnake)]);

        let mut long = world(
            rules(WallMode::Open),
            &[(&[(5, 5), (4, 5)], Right), (&[(6, 5), (7, 5)], Left)],
        );
        assert_eq!(deaths(&mut long), [(0, OtherSnake), (1, OtherSnake)]);
    }

    #[test]
    fn running_into_a_body_kills_only_the_runner() {
        let mut w = world(
            rules(WallMode::Open),
            &[(&[(5, 5), (4, 5)], Right), (&[(6, 4), (6, 5), (6, 6)], Up)],
        );
        assert_eq!(deaths(&mut w), [(0, OtherSnake)]);
        assert_eq!(body(&w, 1), [(6, 3), (6, 4), (6, 5)]);
    }

    #[test]
    fn following_a_tail_that_moves_away_is_safe() {
        let tail_chase: [(&[(u16, u16)], Direction); 2] =
            [(&[(5, 5), (4, 5)], Right), (&[(7, 4), (7, 5), (6, 5)], Up)];
        let mut w = world(rules(WallMode::Open), &tail_chase);
        assert_eq!(deaths(&mut w), []);
        assert_eq!(body(&w, 0), [(6, 5), (5, 5)]);
    }

    #[test]
    fn a_growing_tail_stays_and_kills() {
        // Instant growth leaves the tail cell doubled for a step.
        let mut instant = world(
            rules(WallMode::Open),
            &[
                (&[(5, 5), (4, 5)], Right),
                (&[(7, 4), (7, 5), (6, 5), (6, 5)], Up),
            ],
        );
        assert_eq!(deaths(&mut instant), [(0, OtherSnake)]);

        // At-tail growth that comes due this step keeps the tail too.
        let at_tail = Rules {
            growth_mode: GrowthMode::AtTail,
            ..rules(WallMode::Open)
        };
        let mut w = world(
            at_tail,
            &[(&[(5, 5), (4, 5)], Right), (&[(7, 4), (7, 5), (6, 5)], Up)],
        );
        w.snakes[1].grow(1, GrowthMode::AtTail);
        w.snakes[1].tick_growth();
        w.snakes[1].tick_growth();
        assert_eq!(deaths(&mut w), [(0, OtherSnake)]);
    }

    #[test]
    fn own_tail_follows_the_same_rule() {
        let square: &[(u16, u16)] = &[(5, 5), (5, 6), (6, 6), (6, 5)];
        let mut w = world(rules(WallMode::Open), &[(square, Right)]);
        assert_eq!(deaths(&mut w), [], "the tail moves out of the way");

        let grown: &[(u16, u16)] = &[(5, 5), (5, 6), (6, 6), (6, 5), (6, 5)];
        let mut w = world(rules(WallMode::Open), &[(grown, Right)]);
        assert_eq!(deaths(&mut w), [(0, OwnBody)]);
    }

    #[test]
    fn open_edges_wrap_and_solid_ones_kill() {
        let mut open = world(rules(WallMode::Open), &[(&[(0, 5), (1, 5)], Left)]);
        assert_eq!(deaths(&mut open), []);
        assert_eq!(body(&open, 0), [(29, 5), (0, 5)]);

        let mut solid = world(rules(WallMode::Full), &[(&[(1, 5), (2, 5)], Left)]);
        assert_eq!(deaths(&mut solid), [(0, Wall)]);
    }

    #[test]
    fn pass_through_self_never_passes_through_others() {
        let ghost = Rules {
            pass_through_self: true,
            ..rules(WallMode::Open)
        };
        // Up from (5, 5) lands mid-body at (5, 4).
        let coil: &[(u16, u16)] = &[(5, 5), (6, 5), (6, 4), (5, 4), (4, 4)];
        let mut w = world(rules(WallMode::Open), &[(coil, Up)]);
        assert_eq!(deaths(&mut w), [(0, OwnBody)]);
        let mut w = world(ghost.clone(), &[(coil, Up)]);
        assert_eq!(deaths(&mut w), []);

        let mut w = world(
            ghost,
            &[(&[(5, 5), (4, 5)], Right), (&[(6, 4), (6, 5), (6, 6)], Up)],
        );
        assert_eq!(deaths(&mut w), [(0, OtherSnake)]);
    }

    #[test]
    fn poison_is_eaten_and_kills() {
        let mut w = world(rules(WallMode::Open), &[(&[(5, 5), (4, 5)], Right)]);
        w.place_food(Point { x: 6, y: 5 }, FoodKind::Poison);
        assert_eq!(deaths(&mut w), [(0, Poison)]);
        assert_eq!(body(&w, 0), [(6, 5), (5, 5)], "dies on the poison");
        assert!(w.foods.is_empty());
    }

    #[test]
    fn two_heads_on_one_food_both_die_and_leave_it() {
        let mut w = world(
            rules(WallMode::Open),
            &[(&[(5, 5), (4, 5)], Right), (&[(7, 5), (8, 5)], Left)],
        );
        w.place_food(Point { x: 6, y: 5 }, FoodKind::Normal);
        assert_eq!(deaths(&mut w), [(0, OtherSnake), (1, OtherSnake)]);
        assert_eq!(w.foods.len(), 1);
    }

    #[test]
    fn food_grows_the_snake_that_eats_it() {
        let mut w = world(rules(WallMode::Open), &[(&[(5, 5), (4, 5)], Right)]);
        w.place_food(Point { x: 6, y: 5 }, FoodKind::Super);
        let outcome = w.step();
        assert_eq!(outcome.eaten, [(0, FoodKind::Super)]);
        assert_eq!(w.snakes[0].len(), 2 + 3);
    }

    #[test]
    fn dead_snakes_turn_into_food_when_asked() {
        let mut w = world(
            rules(WallMode::Open),
            &[
                (&[(5, 5), (4, 5), (3, 5)], Right),
                (&[(6, 4), (6, 5), (6, 6)], Up),
            ],
        );
        w.corpses_to_food = true;
        assert_eq!(deaths(&mut w), [(0, OtherSnake)]);
        assert!(w.snakes[0].body.is_empty());
        let mut food: Vec<(u16, u16)> = w.foods.iter().map(|f| (f.pos.x, f.pos.y)).collect();
        food.sort();
        assert_eq!(food, [(3, 5), (4, 5), (5, 5)]);
    }

    /// Runs a multiplayer world with scripted turns: `(tick, snake, dir)`.
    fn run_script(seed: u64, script: &[(usize, usize, Direction)]) -> MatchView {
        let rules = Rules {
            width: 24,
            height: 16,
            inner_walls: InnerWalls::Few,
            food_interval_s: 1,
            ..Rules::default()
        };
        let tick = Duration::from_millis(rules.tick_ms);
        let mut world = World::with_players(rules, seed, 4).unwrap();
        for t in 0..120 {
            for &(_, who, dir) in script.iter().filter(|s| s.0 == t) {
                world.steer(who as u8, dir);
            }
            world.advance_clock(tick);
            world.step();
        }
        world.view()
    }

    #[test]
    fn same_seed_and_inputs_replay_exactly() {
        let script = [
            (3, 0, Up),
            (3, 1, Left),
            (10, 2, Down),
            (11, 3, Right),
            (40, 0, Left),
        ];
        assert_eq!(run_script(42, &script), run_script(42, &script));
        assert_ne!(run_script(42, &script), run_script(43, &script));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn living_snakes_never_overlap(
            seed in any::<u64>(),
            players in 2usize..=4,
            wrap in any::<bool>(),
            keys in prop::collection::vec((0usize..4, 0usize..4), 0..150),
        ) {
            let rules = Rules {
                width: 20,
                height: 14,
                wall_mode: if wrap { WallMode::Open } else { WallMode::Full },
                inner_walls: InnerWalls::Off,
                food_interval_s: 1,
                super_interval_s: 2,
                growth_mode: if seed % 2 == 0 { GrowthMode::Instant } else { GrowthMode::AtTail },
                ..Rules::default()
            };
            let (w, h) = (rules.width, rules.height);
            let world = World::with_players(rules, seed, players);
            prop_assume!(world.is_ok());
            let mut world = world.unwrap();
            let dirs = [Up, Down, Left, Right];
            for (who, dir) in keys {
                if let Some(s) = world.snakes.get_mut(who % players) && s.alive() {
                    s.enqueue(dirs[dir], w, h);
                }
                world.advance_clock(Duration::from_millis(150));
                world.step();

                // Each living snake's cells are distinct, apart from the doubled
                // tail instant growth leaves, and no two snakes share a cell.
                let mut owner: HashMap<Point, u8> = HashMap::new();
                for s in world.snakes.iter().filter(|s| s.alive()) {
                    let cells: Vec<Point> = s.body.iter().copied().collect();
                    let first_repeat = (1..cells.len())
                        .find(|&i| cells[i] == cells[i - 1])
                        .unwrap_or(cells.len());
                    let (distinct, doubled) = cells.split_at(first_repeat);
                    prop_assert!(
                        doubled.iter().all(|&p| p == cells[first_repeat - 1]),
                        "snake {} repeats a cell mid-body", s.id
                    );
                    let mut sorted = distinct.to_vec();
                    sorted.sort_by_key(|p| (p.y, p.x));
                    sorted.dedup();
                    prop_assert_eq!(sorted.len(), distinct.len(), "snake {} overlaps itself", s.id);
                    for &p in distinct {
                        let prev = owner.insert(p, s.id);
                        prop_assert!(prev.is_none(), "snakes {:?} and {} share {:?}", prev, s.id, p);
                    }
                }
            }
        }
    }
}
