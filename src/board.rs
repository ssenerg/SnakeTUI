use std::collections::VecDeque;

use anyhow::{Result, anyhow};
use rand::RngExt;

use crate::config::{GameConfig, WallMode};
use crate::game::{Direction, Point};

pub fn generate_walls(cfg: &GameConfig) -> Vec<bool> {
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
            if no_bottlenecks(&walls, w, h) {
                placed += 1;
            } else {
                walls[i] = false;
            }
        }
    }

    walls
}

/// Whether the free cells form one connected area with no bottleneck: no
/// single free cell whose loss would split it (an articulation point). Such an
/// area has no dead ends and no pocket with only one way in, so the snake can
/// always leave any part of the board it enters.
///
/// Tarjan's algorithm, run as an iterative DFS so big boards can't overflow
/// the stack.
fn no_bottlenecks(walls: &[bool], w: usize, h: usize) -> bool {
    let Some(root) = walls.iter().position(|x| !*x) else {
        return false;
    };
    // Boards are at least 3 cells in each direction, so a cell's four
    // neighbours are distinct and skipping the DFS parent skips one edge.
    let neighbors = |i: usize| {
        let (x, y) = (i % w, i / w);
        [
            y * w + (x + 1) % w,
            y * w + (x + w - 1) % w,
            ((y + 1) % h) * w + x,
            ((y + h - 1) % h) * w + x,
        ]
    };
    // `order`: when the DFS reached each cell (usize::MAX = not yet).
    // `low`: the earliest cell its subtree reaches through one back edge.
    let mut order = vec![usize::MAX; w * h];
    let mut low = vec![0; w * h];
    order[root] = 0;
    let mut seen = 1;
    let mut root_children = 0;
    // (cell, DFS parent, next neighbour to look at)
    let mut stack = vec![(root, root, 0)];
    while let Some(top) = stack.last_mut() {
        let (cell, parent) = (top.0, top.1);
        if top.2 < 4 {
            let next = neighbors(cell)[top.2];
            top.2 += 1;
            if walls[next] || next == parent {
                continue;
            }
            if order[next] == usize::MAX {
                order[next] = seen;
                low[next] = seen;
                seen += 1;
                if cell == root {
                    root_children += 1;
                }
                stack.push((next, cell, 0));
            } else {
                low[cell] = low[cell].min(order[next]);
            }
        } else {
            stack.pop();
            if cell != root {
                low[parent] = low[parent].min(low[cell]);
                // Nothing below `cell` reaches above `parent`, so losing
                // `parent` would cut that part of the board off.
                if parent != root && low[cell] >= order[parent] {
                    return false;
                }
            }
        }
    }
    root_children <= 1 && seen == walls.iter().filter(|x| !**x).count()
}

pub fn place_snake(walls: &[bool], w: u16, h: u16) -> Result<(VecDeque<Point>, Direction)> {
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
                    cands.push(Point {
                        x: cur.x - 1,
                        y: cur.y,
                    });
                }
                if cur.x + 1 < w {
                    cands.push(Point {
                        x: cur.x + 1,
                        y: cur.y,
                    });
                }
                if cur.y > 0 {
                    cands.push(Point {
                        x: cur.x,
                        y: cur.y - 1,
                    });
                }
                if cur.y + 1 < h {
                    cands.push(Point {
                        x: cur.x,
                        y: cur.y + 1,
                    });
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

    // Straightforward checks, independent of the Tarjan code under test.

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

    /// Brute force: some free cell, if walled, splits the rest of the board.
    fn has_bottleneck(walls: &[bool], w: usize, h: usize) -> bool {
        let mut walls = walls.to_vec();
        (0..walls.len()).any(|i| {
            if walls[i] {
                return false;
            }
            walls[i] = true;
            let split = !all_free_connected(&walls, w, h);
            walls[i] = false;
            split
        })
    }

    #[test]
    fn gap_walls_are_mirrored_and_connected() {
        let mut cfg = GameConfig::default();
        cfg.wall_mode = WallMode::Gaps;
        cfg.inner_walls = InnerWalls::Many;
        for _ in 0..20 {
            let (w, h) = (cfg.width as usize, cfg.height as usize);
            let walls = generate_walls(&cfg);
            for x in 0..w {
                assert_eq!(
                    walls[x],
                    walls[(h - 1) * w + x],
                    "top/bottom mirror at x={x}"
                );
            }
            for y in 0..h {
                assert_eq!(
                    walls[y * w],
                    walls[y * w + w - 1],
                    "left/right mirror at y={y}"
                );
            }
            assert!(
                all_free_connected(&walls, w, h),
                "free area must be connected"
            );
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
    fn inner_walls_never_create_bottlenecks() {
        for wall_mode in [WallMode::Full, WallMode::Open, WallMode::Gaps] {
            let cfg = GameConfig {
                wall_mode,
                inner_walls: InnerWalls::Many,
                ..GameConfig::default()
            };
            for _ in 0..20 {
                let (w, h) = (cfg.width as usize, cfg.height as usize);
                let walls = generate_walls(&cfg);
                assert!(
                    !has_bottleneck(&walls, w, h),
                    "a single cell splits the free area"
                );
            }
        }
    }

    #[test]
    fn one_cell_doorway_is_a_bottleneck() {
        let cfg = GameConfig {
            width: 10,
            height: 8,
            ..GameConfig::default()
        };
        let (w, h) = (10, 8);
        let mut walls = generate_walls(&cfg);
        assert!(no_bottlenecks(&walls, w, h));
        // Wall off the left side except for a doorway at (4, 3). Every cell
        // still has two exits, but the room behind the doorway is a trap.
        for y in 1..h - 1 {
            if y != 3 {
                walls[y * w + 4] = true;
            }
        }
        assert!(no_dead_ends(&walls, w, h));
        assert!(!no_bottlenecks(&walls, w, h));
        assert!(has_bottleneck(&walls, w, h));
    }
}
