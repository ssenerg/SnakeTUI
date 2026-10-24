#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

/// The cell one step from `p` in `dir` on a `w`×`h` board, wrapping at the
/// edges. Edge walls, where there are any, stop the snake before it wraps.
pub fn wrap_next(p: Point, dir: Direction, w: u16, h: u16) -> Point {
    match dir {
        Direction::Up => Point {
            x: p.x,
            y: (p.y + h - 1) % h,
        },
        Direction::Down => Point {
            x: p.x,
            y: (p.y + 1) % h,
        },
        Direction::Left => Point {
            x: (p.x + w - 1) % w,
            y: p.y,
        },
        Direction::Right => Point {
            x: (p.x + 1) % w,
            y: p.y,
        },
    }
}
