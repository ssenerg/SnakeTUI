use std::sync::Arc;
use std::time::Duration;

use crate::geom::Point;
use crate::world::FoodKind;

/// Everything needed to draw a board, detached from the [`crate::world::World`]
/// it came from. Cheap to clone: the walls are shared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchView {
    pub width: u16,
    pub height: u16,
    /// Row-major, `width * height` cells.
    pub walls: Arc<[bool]>,
    pub snakes: Vec<SnakeView>,
    pub foods: Vec<(Point, FoodKind)>,
    pub clock: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
