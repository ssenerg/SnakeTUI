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
}
