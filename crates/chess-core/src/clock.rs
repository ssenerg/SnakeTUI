//! The two clocks, kept by the server. Time comes in as how long it's been
//! since the game started, so the clocks never read the time themselves and
//! tests can say exactly when everything happens.
//!
//! Neither clock runs until its side has made a first move: until then a
//! first-move timer runs, and running out of it aborts the game. After
//! that, each move costs its side the time since their turn began, less a
//! lag credit from their measured ping, and earns them the increment.

use std::time::Duration;

use crate::control::TimeControl;
use crate::piece::Color;

/// How long each side has to make their first move.
pub const FIRST_MOVE_TIME: Duration = Duration::from_secs(30);
/// The least a move costs, however quick it was or however laggy its side.
pub const MIN_CHARGE: Duration = Duration::from_millis(10);
/// The most lag one move is credited.
pub const MAX_CREDIT: Duration = Duration::from_millis(500);
/// Lag credit is drawn from a quota: each side starts with this much…
pub const QUOTA_START: Duration = Duration::from_secs(1);
/// …gains this much with each of their moves…
pub const QUOTA_GAIN: Duration = Duration::from_millis(250);
/// …and never holds more than this.
pub const QUOTA_MAX: Duration = Duration::from_secs(2);

/// What ran out, and whose it was.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expiry {
    /// They didn't make their first move in time.
    FirstMove(Color),
    /// Their flag fell.
    Flag(Color),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Clock {
    control: TimeControl,
    /// Each side's time as of the start of the current turn, white first.
    remaining: [Duration; 2],
    quota: [Duration; 2],
    /// Each side's latest measured round trip to the server.
    rtt: [Duration; 2],
    turn: Color,
    /// When the current turn began.
    turn_started: Duration,
    /// Moves made so far, both sides'.
    moves: u32,
    stopped: bool,
}

impl Clock {
    /// Clocks for a game `first` moves first in.
    pub fn new(control: TimeControl, first: Color) -> Clock {
        Clock {
            control,
            remaining: [control.base; 2],
            quota: [QUOTA_START; 2],
            rtt: [Duration::ZERO; 2],
            turn: first,
            turn_started: Duration::ZERO,
            moves: 0,
            stopped: false,
        }
    }

    pub fn control(&self) -> TimeControl {
        self.control
    }

    /// Records `color`'s latest round trip, which their next lag credit
    /// comes from.
    pub fn set_rtt(&mut self, color: Color, rtt: Duration) {
        self.rtt[color.index()] = rtt;
    }

    /// Whose turn it is.
    pub fn turn(&self) -> Color {
        self.turn
    }

    /// Whether both sides have made their first move, so the clocks run.
    pub fn started(&self) -> bool {
        self.moves >= 2
    }

    /// The side whose clock is running: none while first moves are awaited
    /// or once the game is over.
    pub fn running(&self) -> Option<Color> {
        (self.started() && !self.stopped).then_some(self.turn)
    }

    /// When the side to move must make their first move by, while first
    /// moves are awaited.
    pub fn first_move_deadline(&self) -> Option<Duration> {
        (!self.started() && !self.stopped).then_some(self.turn_started + FIRST_MOVE_TIME)
    }

    /// `color`'s time left at `at`, counting down by the wall clock: lag
    /// credit, given when they move, may hand some of it back.
    pub fn remaining(&self, color: Color, at: Duration) -> Duration {
        let left = self.remaining[color.index()];
        if self.running() == Some(color) {
            left.saturating_sub(at.saturating_sub(self.turn_started))
        } else {
            left
        }
    }

    /// The lag credit `color`'s next move will get: their round trip, up to
    /// [`MAX_CREDIT`] and what's left of their quota.
    pub fn credit(&self, color: Color) -> Duration {
        let i = color.index();
        self.rtt[i].min(MAX_CREDIT).min(self.quota[i])
    }

    /// What a move by the side to move at `at` would cost them.
    fn charge(&self, at: Duration) -> Duration {
        let elapsed = at.saturating_sub(self.turn_started);
        elapsed
            .saturating_sub(self.credit(self.turn))
            .max(MIN_CHARGE)
    }

    /// What has run out by `at`, if anything: the side to move's first-move
    /// timer, or their flag, which falls when a move then would cost them
    /// all their time.
    pub fn expiry(&self, at: Duration) -> Option<Expiry> {
        if self.stopped {
            None
        } else if !self.started() {
            let late = at.saturating_sub(self.turn_started) >= FIRST_MOVE_TIME;
            late.then_some(Expiry::FirstMove(self.turn))
        } else {
            let flag = self.charge(at) >= self.remaining[self.turn.index()];
            flag.then_some(Expiry::Flag(self.turn))
        }
    }

    /// How long after `at` something runs out if nobody moves, for the
    /// server to wake up then; zero if it already has, `None` once stopped.
    pub fn until_next(&self, at: Duration) -> Option<Duration> {
        if self.stopped {
            return None;
        }
        let deadline = if !self.started() {
            self.turn_started + FIRST_MOVE_TIME
        } else {
            let left = self.remaining[self.turn.index()];
            if left <= MIN_CHARGE {
                return Some(Duration::ZERO);
            }
            self.turn_started + left + self.credit(self.turn)
        };
        Some(deadline.saturating_sub(at))
    }

    /// The side to move moves at `at`. Returns what it cost them, nothing
    /// for a first move, or what had run out first, leaving the clocks as
    /// they were.
    pub fn on_move(&mut self, at: Duration) -> Result<Duration, Expiry> {
        if let Some(expiry) = self.expiry(at) {
            return Err(expiry);
        }
        let i = self.turn.index();
        let mut charged = Duration::ZERO;
        if self.started() {
            charged = self.charge(at);
            let credited = at.saturating_sub(self.turn_started).saturating_sub(charged);
            self.quota[i] = (self.quota[i] - credited + QUOTA_GAIN).min(QUOTA_MAX);
            self.remaining[i] = self.remaining[i] - charged + self.control.increment;
        }
        self.moves += 1;
        self.turn = !self.turn;
        self.turn_started = at;
        Ok(charged)
    }

    /// Stops both clocks at `at`, when the game ends. The side to move
    /// keeps what the wall clock left them, or nothing if their flag fell.
    pub fn stop(&mut self, at: Duration) {
        if self.stopped {
            return;
        }
        if self.started() {
            let flagged = matches!(self.expiry(at), Some(Expiry::Flag(_)));
            let left = self.remaining(self.turn, at);
            self.remaining[self.turn.index()] = if flagged { Duration::ZERO } else { left };
        }
        self.stopped = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Color = Color::White;
    const BLACK: Color = Color::Black;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// A 3+2 clock past both first moves, made at 1 s and 2 s, so white's
    /// clock started at 2 s.
    fn started() -> Clock {
        let mut clock = Clock::new(TimeControl::minutes(3, 2), WHITE);
        clock.on_move(secs(1)).unwrap();
        clock.on_move(secs(2)).unwrap();
        clock
    }

    #[test]
    fn first_moves_cost_nothing_and_start_the_clocks() {
        let mut clock = Clock::new(TimeControl::minutes(3, 2), WHITE);
        assert_eq!(clock.running(), None);
        assert_eq!(clock.first_move_deadline(), Some(secs(30)));
        assert_eq!(clock.on_move(secs(20)), Ok(Duration::ZERO));
        assert_eq!(clock.first_move_deadline(), Some(secs(50)));
        assert_eq!(clock.running(), None);
        assert_eq!(clock.on_move(secs(45)), Ok(Duration::ZERO));
        assert_eq!(clock.first_move_deadline(), None);
        assert_eq!(clock.running(), Some(WHITE));
        assert_eq!(clock.remaining(WHITE, secs(45)), secs(180));
        assert_eq!(clock.remaining(BLACK, secs(45)), secs(180));
        assert_eq!(clock.remaining(WHITE, secs(55)), secs(170));
    }

    #[test]
    fn each_side_has_30_seconds_for_a_first_move() {
        let mut clock = Clock::new(TimeControl::minutes(1, 0), WHITE);
        assert_eq!(clock.expiry(ms(29_999)), None);
        assert_eq!(clock.until_next(secs(10)), Some(secs(20)));
        assert_eq!(clock.expiry(secs(30)), Some(Expiry::FirstMove(WHITE)));
        assert_eq!(clock.on_move(secs(30)), Err(Expiry::FirstMove(WHITE)));

        clock.on_move(secs(10)).unwrap();
        assert_eq!(clock.expiry(secs(39)), None);
        assert_eq!(clock.expiry(secs(40)), Some(Expiry::FirstMove(BLACK)));
        assert_eq!(clock.until_next(secs(25)), Some(secs(15)));
    }

    #[test]
    fn a_move_costs_its_time_less_lag_credit_and_earns_the_increment() {
        let mut clock = started();
        clock.set_rtt(WHITE, ms(200));
        assert_eq!(clock.on_move(secs(5)), Ok(ms(2800)));
        assert_eq!(clock.remaining(WHITE, secs(5)), ms(180_000 - 2800 + 2000));
        // 200 ms of the 1 s quota went, and 250 ms came back.
        assert_eq!(clock.quota[0], ms(1050));
        assert_eq!(clock.running(), Some(BLACK));

        // Black has no lag: every millisecond counts.
        assert_eq!(clock.on_move(ms(9500)), Ok(ms(4500)));
        assert_eq!(clock.remaining(BLACK, secs(10)), ms(180_000 - 4500 + 2000));
    }

    #[test]
    fn credit_is_at_most_half_a_second() {
        let mut clock = started();
        clock.set_rtt(WHITE, ms(800));
        assert_eq!(clock.credit(WHITE), ms(500));
        assert_eq!(clock.on_move(secs(5)), Ok(ms(2500)));
    }

    #[test]
    fn credit_is_drawn_from_a_quota_that_refills_slowly() {
        let mut clock = started();
        clock.set_rtt(WHITE, ms(500));
        let mut at = secs(2);
        let mut credits = Vec::new();
        for _ in 0..5 {
            credits.push(clock.credit(WHITE));
            at += secs(3);
            clock.on_move(at).unwrap();
            at += secs(1);
            clock.on_move(at).unwrap();
        }
        // The quota goes from 1 s to 750 ms, 500 ms and 250 ms, where the
        // 250 ms each move earns holds it.
        assert_eq!(credits, [ms(500), ms(500), ms(500), ms(250), ms(250)]);
    }

    #[test]
    fn the_quota_holds_at_most_two_seconds() {
        let mut clock = started();
        let mut at = secs(2);
        for _ in 0..10 {
            at += secs(1);
            clock.on_move(at).unwrap();
            at += secs(1);
            clock.on_move(at).unwrap();
        }
        assert_eq!(clock.quota, [QUOTA_MAX; 2]);
    }

    #[test]
    fn a_move_costs_at_least_10_ms() {
        let mut clock = started();
        assert_eq!(clock.on_move(ms(2005)), Ok(ms(10)));
        // A laggy side is credited only what its move needed.
        clock.set_rtt(BLACK, ms(400));
        assert_eq!(clock.on_move(ms(2305)), Ok(ms(10)));
        assert_eq!(clock.quota[1], ms(1000 - 290 + 250));
    }

    #[test]
    fn a_flag_falls_when_a_move_would_cost_all_the_time_left() {
        let mut clock = started();
        let falls = secs(2 + 180);
        assert_eq!(clock.until_next(secs(2)), Some(secs(180)));
        assert_eq!(clock.expiry(falls - ms(1)), None);
        assert_eq!(clock.expiry(falls), Some(Expiry::Flag(WHITE)));
        assert_eq!(clock.on_move(falls), Err(Expiry::Flag(WHITE)));
        assert_eq!(clock.remaining(WHITE, falls), Duration::ZERO);

        // Lag credit holds the flag up as long as a move would be credited.
        clock.set_rtt(WHITE, ms(300));
        assert_eq!(clock.until_next(secs(2)), Some(ms(180_300)));
        assert_eq!(clock.expiry(falls + ms(299)), None);
        assert_eq!(clock.expiry(falls + ms(300)), Some(Expiry::Flag(WHITE)));
        assert_eq!(clock.on_move(falls + ms(200)), Ok(ms(179_900)));
        assert_eq!(clock.remaining(WHITE, falls), ms(100 + 2000));
    }

    #[test]
    fn with_10_ms_or_less_left_the_flag_falls_at_once() {
        let mut clock = started();
        clock.remaining[0] = ms(10);
        assert_eq!(clock.until_next(secs(2)), Some(Duration::ZERO));
        assert_eq!(clock.expiry(secs(2)), Some(Expiry::Flag(WHITE)));
        clock.remaining[0] = ms(11);
        assert_eq!(clock.expiry(secs(2)), None);
    }

    #[test]
    fn stopping_freezes_both_clocks() {
        let mut clock = started();
        clock.on_move(secs(12)).unwrap();
        clock.stop(secs(20));
        assert_eq!(clock.running(), None);
        assert_eq!(clock.until_next(secs(20)), None);
        assert_eq!(clock.expiry(secs(1000)), None);
        assert_eq!(clock.remaining(WHITE, secs(1000)), secs(180 - 10 + 2));
        assert_eq!(clock.remaining(BLACK, secs(1000)), secs(180 - 8));

        let mut flagged = started();
        flagged.remaining[0] = ms(5);
        flagged.stop(secs(2));
        assert_eq!(flagged.remaining(WHITE, secs(2)), Duration::ZERO);
    }

    #[test]
    fn black_may_move_first() {
        let mut clock = Clock::new(TimeControl::minutes(1, 0), BLACK);
        assert_eq!(clock.expiry(secs(30)), Some(Expiry::FirstMove(BLACK)));
        clock.on_move(secs(1)).unwrap();
        clock.on_move(secs(2)).unwrap();
        assert_eq!(clock.running(), Some(BLACK));
    }
}
