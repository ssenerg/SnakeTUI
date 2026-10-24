use std::sync::Arc;
use std::time::Duration;

use crate::geom::Point;
use crate::world::FoodKind;

/// Everything needed to draw a board, detached from the [`crate::world::World`]
/// it came from. Cheap to clone: the walls are shared.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MatchView {
    pub width: u16,
    pub height: u16,
    /// Row-major, `width * height` cells.
    #[cfg_attr(feature = "serde", serde(with = "packed"))]
    pub walls: Arc<[bool]>,
    pub snakes: Vec<SnakeView>,
    pub foods: Vec<(Point, FoodKind)>,
    pub clock: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SnakeView {
    pub id: u8,
    /// Head first; empty once a dead snake has turned into food.
    pub body: Vec<Point>,
    pub alive: bool,
}

impl MatchView {
    pub fn wall_at(&self, x: u16, y: u16) -> bool {
        self.walls[y as usize * self.width as usize + x as usize]
    }
}

/// Walls on the wire take one bit a cell: a 30×18 board's 540 cells fit in
/// 68 bytes.
#[cfg(feature = "serde")]
mod packed {
    use std::sync::Arc;

    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// More cells than any board can have.
    const MAX_CELLS: usize = 1 << 20;

    pub fn serialize<S: Serializer>(cells: &Arc<[bool]>, s: S) -> Result<S::Ok, S::Error> {
        let mut bits = vec![0u8; cells.len().div_ceil(8)];
        for (i, _) in cells.iter().enumerate().filter(|(_, wall)| **wall) {
            bits[i / 8] |= 1 << (i % 8);
        }
        (cells.len() as u32, bits).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<[bool]>, D::Error> {
        let (len, bits) = <(u32, Vec<u8>)>::deserialize(d)?;
        let len = len as usize;
        if len > MAX_CELLS || bits.len() != len.div_ceil(8) {
            return Err(D::Error::custom("walls of the wrong size"));
        }
        Ok((0..len)
            .map(|i| bits[i / 8] & (1 << (i % 8)) != 0)
            .collect())
    }
}
