use std::collections::VecDeque;

use crate::config::GrowthMode;
use crate::geom::{Direction, Point, wrap_next};

/// How many turns a snake can have queued for upcoming ticks.
pub const QUEUE_CAP: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeathCause {
    Wall,
    OwnBody,
    /// Another snake's head or body, including head-on crashes.
    OtherSnake,
    Poison,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snake {
    pub id: u8,
    /// Head first. After instant growth the tail cell repeats until the snake
    /// has moved far enough to fill it.
    pub body: VecDeque<Point>,
    pub dir: Direction,
    /// Turns waiting for upcoming ticks, oldest first.
    queue: VecDeque<Direction>,
    /// At-tail growth: steps left until each pending segment is added.
    growth: Vec<u32>,
    pub death: Option<DeathCause>,
}

impl Snake {
    pub fn new(id: u8, body: VecDeque<Point>, dir: Direction) -> Self {
        Self {
            id,
            body,
            dir,
            queue: VecDeque::new(),
            growth: Vec::new(),
            death: None,
        }
    }

    pub fn head(&self) -> Point {
        self.body[0]
    }

    /// The snake's length, counting segments that haven't filled in yet.
    #[expect(clippy::len_without_is_empty, reason = "a snake is never empty")]
    pub fn len(&self) -> usize {
        self.body.len()
    }

    pub fn alive(&self) -> bool {
        self.death.is_none()
    }

    /// The first body cell behind the head. A one-cell snake has none, even
    /// right after growing, and may turn any way.
    pub fn neck(&self) -> Option<Point> {
        let head = self.head();
        self.body.iter().copied().find(|&p| p != head)
    }

    /// Whether moving `dir` would go straight back into the neck.
    pub fn reverses(&self, dir: Direction, w: u16, h: u16) -> bool {
        self.neck() == Some(wrap_next(self.head(), dir, w, h))
    }

    /// Turns right away (instant moves). Returns `false` for a reversal, or
    /// if the snake is dead.
    pub fn set_dir(&mut self, dir: Direction, w: u16, h: u16) -> bool {
        if !self.alive() || self.reverses(dir, w, h) {
            return false;
        }
        self.dir = dir;
        self.queue.clear();
        true
    }

    /// Queues a turn for an upcoming tick. Returns `false` when the key is
    /// dropped: a reversal, a full queue, or a dead snake. Repeating the
    /// direction the snake will already be heading is accepted but doesn't
    /// take a queue slot.
    pub fn enqueue(&mut self, dir: Direction, w: u16, h: u16) -> bool {
        if !self.alive() {
            return false;
        }
        let reverses = match self.queue.back() {
            None => {
                if dir == self.dir {
                    return true;
                }
                self.reverses(dir, w, h)
            }
            Some(&last) => {
                if dir == last {
                    return true;
                }
                // After its next move, any snake longer than one cell has a neck.
                dir == last.opposite() && self.len() > 1
            }
        };
        if reverses || self.queue.len() >= QUEUE_CAP {
            return false;
        }
        self.queue.push_back(dir);
        true
    }

    /// Takes the next queued turn, skipping any that would now reverse.
    pub(crate) fn take_turn(&mut self, w: u16, h: u16) {
        while let Some(dir) = self.queue.pop_front() {
            if !self.reverses(dir, w, h) {
                self.dir = dir;
                return;
            }
        }
    }

    /// Counts down at-tail growth by one step. Returns whether the tail stays
    /// put this step to add a segment.
    pub(crate) fn tick_growth(&mut self) -> bool {
        for t in self.growth.iter_mut() {
            *t = t.saturating_sub(1);
        }
        match self.growth.iter().position(|&t| t == 0) {
            Some(i) => {
                self.growth.remove(i);
                true
            }
            None => false,
        }
    }

    /// Grows by `n` segments after eating, at once or when the food reaches
    /// the tail.
    pub(crate) fn grow(&mut self, n: u32, mode: GrowthMode) {
        match mode {
            GrowthMode::Instant => {
                let tail = *self.body.back().unwrap();
                for _ in 0..n {
                    self.body.push_back(tail);
                }
            }
            GrowthMode::AtTail => {
                let len = self.len() as u32;
                for _ in 0..n {
                    self.growth.push(len);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u16 = 30;
    const H: u16 = 20;

    fn snake(cells: &[(u16, u16)], dir: Direction) -> Snake {
        let body = cells.iter().map(|&(x, y)| Point { x, y }).collect();
        Snake::new(0, body, dir)
    }

    #[test]
    fn a_grown_one_cell_snake_has_no_neck() {
        let mut s = snake(&[(5, 5)], Direction::Right);
        s.grow(1, GrowthMode::Instant);
        assert_eq!(s.len(), 2);
        assert_eq!(s.neck(), None);
        assert!(s.set_dir(Direction::Left, W, H), "nothing is behind it");
    }

    #[test]
    fn a_longer_snake_cannot_reverse() {
        let mut s = snake(&[(5, 5), (4, 5)], Direction::Right);
        assert!(!s.set_dir(Direction::Left, W, H));
        assert!(s.set_dir(Direction::Up, W, H));
    }

    #[test]
    fn queue_drops_reversals_repeats_and_overflow() {
        let mut s = snake(&[(5, 5), (4, 5), (3, 5)], Direction::Right);
        assert!(s.enqueue(Direction::Right, W, H), "already heading right");
        assert!(!s.enqueue(Direction::Left, W, H), "reversal");
        assert!(s.enqueue(Direction::Up, W, H));
        assert!(!s.enqueue(Direction::Down, W, H), "reverses the queued Up");
        assert!(s.enqueue(Direction::Left, W, H));
        assert!(s.enqueue(Direction::Down, W, H));
        assert!(!s.enqueue(Direction::Right, W, H), "queue is full");
        assert_eq!(s.queue, [Direction::Up, Direction::Left, Direction::Down]);
    }

    #[test]
    fn at_tail_growth_waits_for_the_tail() {
        let mut s = snake(&[(5, 5), (4, 5), (3, 5)], Direction::Right);
        s.grow(2, GrowthMode::AtTail);
        assert!(!s.tick_growth());
        assert!(!s.tick_growth());
        assert!(s.tick_growth(), "third step: the food reached the tail");
        assert!(s.tick_growth(), "second segment follows right after");
        assert!(!s.tick_growth());
    }
}
