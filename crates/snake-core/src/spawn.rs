//! Starting positions for several snakes on one board.

use std::collections::VecDeque;

use anyhow::{Result, bail};
use rand::Rng;
use rand::seq::SliceRandom;

use crate::geom::{Direction, Point, wrap_next};

/// Each snake starts this long, in a straight line.
pub const START_LEN: usize = 3;
/// Free cells each snake has straight ahead of its head at the start.
pub const CLEAR_AHEAD: usize = 5;
/// Minimum distance between any two snakes' starting bodies.
pub const MIN_GAP: u32 = 6;

const DIRS: [Direction; 4] = [
    Direction::Up,
    Direction::Down,
    Direction::Left,
    Direction::Right,
];

struct Candidate {
    /// Head first.
    body: Vec<Point>,
    dir: Direction,
    /// The body plus the cells ahead of the head.
    zone: Vec<Point>,
}

/// Shortest distance between two cells, allowing the board to wrap. On
/// walled boards that can only underestimate, which keeps snakes further
/// apart, never closer.
fn distance(a: Point, b: Point, w: u16, h: u16) -> u32 {
    let dx = a.x.abs_diff(b.x);
    let dy = a.y.abs_diff(b.y);
    u32::from(dx.min(w - dx)) + u32::from(dy.min(h - dy))
}

fn min_distance(a: &[Point], b: &[Point], w: u16, h: u16) -> u32 {
    a.iter()
        .flat_map(|&p| b.iter().map(move |&q| distance(p, q, w, h)))
        .min()
        .unwrap_or(u32::MAX)
}

/// `n` cells in a line, starting at `from` and stepping `dir`.
fn line(from: Point, dir: Direction, n: usize, w: u16, h: u16) -> Vec<Point> {
    std::iter::successors(Some(from), |&p| Some(wrap_next(p, dir, w, h)))
        .take(n)
        .collect()
}

fn candidates(walls: &[bool], w: u16, h: u16) -> Vec<Candidate> {
    let free = |p: &Point| !walls[p.y as usize * w as usize + p.x as usize];
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let head = Point { x, y };
            for dir in DIRS {
                let body = line(head, dir.opposite(), START_LEN, w, h);
                let ahead = &line(head, dir, CLEAR_AHEAD + 1, w, h)[1..];
                let zone: Vec<Point> = body.iter().chain(ahead).copied().collect();
                let mut cells = zone.clone();
                cells.sort_by_key(|p| (p.y, p.x));
                cells.dedup();
                // Distinct cells, so the line doesn't wrap onto itself.
                if cells.len() == zone.len() && zone.iter().all(free) {
                    out.push(Candidate { body, dir, zone });
                }
            }
        }
    }
    out
}

/// Places `n` snakes: straight [`START_LEN`]-cell bodies with [`CLEAR_AHEAD`]
/// free cells in front, at least [`MIN_GAP`] apart, and no snake starting in
/// another's path. Each new snake goes as far from the others as possible.
pub fn spawn_points<R: Rng + ?Sized>(
    walls: &[bool],
    w: u16,
    h: u16,
    n: usize,
    rng: &mut R,
) -> Result<Vec<(VecDeque<Point>, Direction)>> {
    let mut pool = candidates(walls, w, h);
    // Shuffled, so the first snake lands anywhere and ties go either way.
    pool.shuffle(rng);
    let mut chosen: Vec<Candidate> = Vec::with_capacity(n);
    while chosen.len() < n {
        let best = pool
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                chosen.iter().all(|o| {
                    min_distance(&c.zone, &o.zone, w, h) >= 2
                        && min_distance(&c.body, &o.body, w, h) >= MIN_GAP
                })
            })
            .max_by_key(|(_, c)| {
                chosen
                    .iter()
                    .map(|o| min_distance(&c.zone, &o.zone, w, h))
                    .min()
                    .unwrap_or(0)
            })
            .map(|(i, _)| i);
        let Some(i) = best else {
            bail!("only room for {} of {n} snakes on this board", chosen.len());
        };
        chosen.push(pool.swap_remove(i));
    }
    Ok(chosen
        .into_iter()
        .map(|c| (VecDeque::from(c.body), c.dir))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::generate_walls;
    use crate::config::{InnerWalls, Rules, WallMode};
    use rand::SeedableRng;
    use rand::rngs::Xoshiro256PlusPlus;

    #[test]
    fn eight_snakes_fit_apart_with_room_ahead() {
        for (seed, wall_mode, inner_walls) in [
            (1, WallMode::Full, InnerWalls::Off),
            (2, WallMode::Open, InnerWalls::Few),
            (3, WallMode::Gaps, InnerWalls::Medium),
        ] {
            let rules = Rules {
                width: 30,
                height: 18,
                wall_mode,
                inner_walls,
                ..Rules::default()
            };
            let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed);
            let walls = generate_walls(&rules, &mut rng);
            let (w, h) = (rules.width, rules.height);
            let snakes = spawn_points(&walls, w, h, 8, &mut rng).unwrap();
            assert_eq!(snakes.len(), 8);
            for (i, (body, dir)) in snakes.iter().enumerate() {
                let free = |p: &Point| !walls[p.y as usize * w as usize + p.x as usize];
                assert!(body.iter().all(free));
                let mut p = body[0];
                for _ in 0..CLEAR_AHEAD {
                    p = wrap_next(p, *dir, w, h);
                    assert!(free(&p), "snake {i} has a wall ahead");
                    assert!(
                        snakes.iter().all(|(b, _)| !b.contains(&p)),
                        "snake in the way"
                    );
                }
                for (other, _) in &snakes[i + 1..] {
                    let (a, b): (Vec<_>, Vec<_>) = (
                        body.iter().copied().collect(),
                        other.iter().copied().collect(),
                    );
                    let gap = min_distance(&a, &b, w, h);
                    assert!(gap >= MIN_GAP, "snakes {gap} apart");
                }
            }
        }
    }

    #[test]
    fn too_many_snakes_is_an_error() {
        let rules = Rules {
            width: 10,
            height: 8,
            ..Rules::default()
        };
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(7);
        let walls = generate_walls(&rules, &mut rng);
        assert!(spawn_points(&walls, 10, 8, 8, &mut rng).is_err());
    }
}
