//! A computer player. Each step it heads for the nearest food it can reach,
//! never into a wall, a body or poison while it has another way, and away
//! from moves that would box it in or meet another head.

use std::collections::VecDeque;

use crate::geom::{Direction, Point, wrap_next};
use crate::world::{FoodKind, World};

const DIRECTIONS: [Direction; 4] = [
    Direction::Up,
    Direction::Down,
    Direction::Left,
    Direction::Right,
];

/// Which way snake `id` should go next on `world`; `None` if it's dead.
pub fn choose(world: &World, id: u8) -> Option<Direction> {
    let me = world.snakes.iter().find(|s| s.id == id && s.alive())?;
    let (w, h) = (world.width, world.height);
    let at = |p: Point| p.y as usize * w as usize + p.x as usize;

    // Cells that kill whoever moves in next step: walls, bodies and poison.
    // A tail moves away, unless the snake is growing and its last cell
    // repeats.
    let mut blocked = vec![false; w as usize * h as usize];
    for y in 0..h {
        for x in 0..w {
            blocked[at(Point { x, y })] = world.wall_at(x, y);
        }
    }
    for snake in world.snakes.iter().filter(|s| s.alive()) {
        if snake.id == id && world.rules.pass_through_self {
            continue;
        }
        let mut body: Vec<Point> = snake.body.iter().copied().collect();
        if body.len() > 1 && body[body.len() - 1] != body[body.len() - 2] {
            body.pop();
        }
        for p in body {
            blocked[at(p)] = true;
        }
    }
    for food in world.foods.iter().filter(|f| f.kind == FoodKind::Poison) {
        blocked[at(food.pos)] = true;
    }
    // Where other heads may land next step: a head-on crash kills both.
    let mut contested = vec![false; blocked.len()];
    for snake in world.snakes.iter().filter(|s| s.alive() && s.id != id) {
        for dir in DIRECTIONS {
            contested[at(wrap_next(snake.head(), dir, w, h))] = true;
        }
    }
    let mut food = vec![false; blocked.len()];
    for f in world.foods.iter().filter(|f| f.kind != FoodKind::Poison) {
        food[at(f.pos)] = true;
    }

    DIRECTIONS
        .into_iter()
        .filter(|&dir| !me.reverses(dir, w, h))
        .max_by_key(|&dir| {
            let next = wrap_next(me.head(), dir, w, h);
            let safe = !blocked[at(next)];
            let (room, to_food) = if safe {
                explore(next, w, h, &blocked, &food)
            } else {
                (0, None)
            };
            // Most important first.
            (
                safe,
                room >= me.len(),
                !contested[at(next)],
                std::cmp::Reverse(to_food.unwrap_or(usize::MAX)),
                room,
                dir == me.dir,
            )
        })
}

/// From `start`: how many open cells can be reached, and how many steps
/// away the nearest food is.
fn explore(
    start: Point,
    w: u16,
    h: u16,
    blocked: &[bool],
    food: &[bool],
) -> (usize, Option<usize>) {
    let at = |p: Point| p.y as usize * w as usize + p.x as usize;
    let mut seen = vec![false; blocked.len()];
    let mut queue = VecDeque::from([(start, 0)]);
    seen[at(start)] = true;
    let (mut room, mut nearest) = (0, None);
    while let Some((p, steps)) = queue.pop_front() {
        room += 1;
        if nearest.is_none() && food[at(p)] {
            nearest = Some(steps);
        }
        for dir in DIRECTIONS {
            let next = wrap_next(p, dir, w, h);
            if !blocked[at(next)] && !seen[at(next)] {
                seen[at(next)] = true;
                queue.push_back((next, steps + 1));
            }
        }
    }
    (room, nearest)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::config::{Rules, WallMode};
    use crate::snake::Snake;
    use crate::world::Food;

    fn p(x: u16, y: u16) -> Point {
        Point { x, y }
    }

    /// A walled 12×10 board with one snake heading right from (5, 5), and
    /// no food unless a test adds some.
    fn board(rules: Rules) -> World {
        let rules = Rules {
            width: 12,
            height: 10,
            food_interval_s: 0,
            super_interval_s: 0,
            poison_interval_s: 0,
            ..rules
        };
        let mut world = World::with_players(rules, 1, 2).unwrap();
        world.snakes.truncate(1);
        world.snakes[0] = Snake::new(
            0,
            VecDeque::from([p(5, 5), p(4, 5), p(3, 5)]),
            Direction::Right,
        );
        world.foods.clear();
        world
    }

    fn food(world: &mut World, pos: Point, kind: FoodKind) {
        world.foods.push(Food {
            pos,
            kind,
            expires_at: None,
        });
    }

    #[test]
    fn it_heads_for_the_nearest_food() {
        let mut world = board(Rules::default());
        food(&mut world, p(5, 2), FoodKind::Normal);
        food(&mut world, p(9, 9), FoodKind::Super);
        assert_eq!(choose(&world, 0), Some(Direction::Up));
        world.foods.clear();
        food(&mut world, p(5, 8), FoodKind::Normal);
        assert_eq!(choose(&world, 0), Some(Direction::Down));
    }

    #[test]
    fn it_never_turns_back_or_into_a_wall_or_poison() {
        let mut world = board(Rules::default());
        // Food behind it can't pull it into its own neck.
        food(&mut world, p(1, 5), FoodKind::Normal);
        assert_ne!(choose(&world, 0), Some(Direction::Left));

        // At the right-hand wall, heading into it.
        world.snakes[0].body = VecDeque::from([p(10, 5), p(9, 5), p(8, 5)]);
        world.foods.clear();
        food(&mut world, p(10, 4), FoodKind::Poison);
        assert_eq!(choose(&world, 0), Some(Direction::Down));
    }

    #[test]
    fn it_wraps_when_the_edges_are_open() {
        let mut world = board(Rules {
            wall_mode: WallMode::Open,
            ..Rules::default()
        });
        world.snakes[0].body = VecDeque::from([p(11, 5), p(10, 5), p(9, 5)]);
        food(&mut world, p(1, 5), FoodKind::Normal);
        assert_eq!(
            choose(&world, 0),
            Some(Direction::Right),
            "through the edge"
        );
    }

    #[test]
    fn it_keeps_out_of_dead_ends() {
        let mut world = board(Rules::default());
        world.snakes[0].body = (1..=5).rev().map(|x| p(x, 5)).collect();
        // Another snake walls in a two-cell pocket above, with food in it.
        // Its tail repeats, so it stays put next step.
        let pocket = [
            (4, 4),
            (4, 3),
            (4, 2),
            (5, 2),
            (6, 2),
            (6, 3),
            (6, 4),
            (6, 4),
        ];
        let other = pocket.into_iter().map(|(x, y)| p(x, y)).collect();
        world.snakes.push(Snake::new(1, other, Direction::Up));
        food(&mut world, p(5, 3), FoodKind::Normal);
        assert_eq!(
            choose(&world, 0),
            Some(Direction::Right),
            "too small for it"
        );
    }

    #[test]
    fn it_has_no_move_once_dead() {
        let mut world = board(Rules::default());
        world.snakes[0].death = Some(crate::snake::DeathCause::Wall);
        assert_eq!(choose(&world, 0), None);
    }

    #[test]
    fn four_bots_last_on_a_quick_play_board() {
        for seed in 0..6 {
            let mut world = World::with_players(crate::round::quick_rules(), seed, 4).unwrap();
            let mut steps = 0;
            while world.alive() == 4 && steps < 2000 {
                world.advance_clock(std::time::Duration::from_millis(150));
                for id in 0..4 {
                    if let Some(dir) = choose(&world, id) {
                        world.steer(id, dir);
                    }
                }
                world.step();
                steps += 1;
            }
            assert!(steps >= 50, "seed {seed}: a bot died after {steps} steps");
        }
    }
}
