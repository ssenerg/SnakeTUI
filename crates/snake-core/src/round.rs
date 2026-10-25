//! A multiplayer room's rounds: players gather, a countdown runs, everyone
//! plays on one board, and the results show. A matched room plays one round;
//! a private room plays round after round.
//!
//! A [`Round`] only moves when told to ([`Round::advance`]), so tests drive
//! its clock by hand, and a seed replays the same games.

use std::time::Duration;

use rand::rngs::Xoshiro256PlusPlus;
use rand::{Rng, SeedableRng};

use crate::bot;
use crate::config::{MoveTiming, Rules};
use crate::geom::Direction;
use crate::settings::rules_in_range;
use crate::view::MatchView;
use crate::world::World;

/// Identifies a player to a room. The server hands them out; the round
/// never interprets them.
pub type PlayerId = u64;

pub const MIN_PLAYERS: usize = 2;
pub const MAX_PLAYERS: usize = 8;

/// Bots' ids have this bit set; the server never gives it to a person.
pub const BOT: PlayerId = 1 << 63;
/// Bots are named after snakes, one name for every seat but a person's.
const BOT_NAMES: [&str; MAX_PLAYERS - 1] = [
    "Viper", "Cobra", "Mamba", "Krait", "Taipan", "Adder", "Python",
];

/// Whether this player is a bot.
pub fn is_bot(id: PlayerId) -> bool {
    id & BOT != 0
}

/// The room's bot number `n`, from 0.
pub fn bot_entrant(n: usize) -> Entrant {
    Entrant {
        id: BOT | n as PlayerId,
        name: BOT_NAMES[n % BOT_NAMES.len()].to_string(),
    }
}

/// Every bot's name, which accounts can't take.
pub fn bot_names() -> &'static [&'static str] {
    &BOT_NAMES
}

/// The board every quick-play match uses. With the scoreboard beside it, it
/// fits an 80×23 terminal (80×24 with a tmux status bar).
pub fn quick_rules() -> Rules {
    Rules {
        width: 30,
        height: 18,
        ..Rules::default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RoomKind {
    /// Formed by the matchmaker: one round with the players it picked.
    Match,
    /// Made by a player and joined by code: rounds until everyone leaves.
    Private,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundSettings {
    pub rules: Rules,
    pub kind: RoomKind,
    pub max_players: usize,
    /// Private rooms: how long after the second player arrives the countdown
    /// starts by itself.
    pub start_after: Duration,
    pub countdown: Duration,
    pub time_limit: Duration,
    /// Private rooms: how long the results show before the next round.
    pub results_for: Duration,
}

impl RoundSettings {
    /// `rules` made fit for multiplayer: moves on the tick and no boost.
    /// `None` when the board can't fit two snakes.
    pub fn new(mut rules: Rules, kind: RoomKind) -> Option<Self> {
        rules.move_timing = MoveTiming::OnTick;
        rules.boost_duration_s = 0;
        let max_players = max_players_for(&rules)?;
        Some(Self {
            rules,
            kind,
            max_players,
            start_after: Duration::from_secs(15),
            countdown: Duration::from_secs(3),
            time_limit: Duration::from_secs(180),
            results_for: Duration::from_secs(8),
        })
    }
}

/// How many snakes boards with these rules have room for, found by placing
/// them on a few boards. `None` if not even [`MIN_PLAYERS`] fit.
pub fn max_players_for(rules: &Rules) -> Option<usize> {
    (MIN_PLAYERS..=MAX_PLAYERS)
        .rev()
        .find(|&n| (0..4).all(|seed| World::with_players(rules.clone(), seed, n).is_ok()))
}

/// A player as the room shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Entrant {
    pub id: PlayerId,
    /// Already safe to draw: the server checked it.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Outcome {
    /// The last snake alive, or the longest one when time ran out.
    Won { winner: Entrant, time_up: bool },
    /// Everyone left died on the same tick, or the longest snakes tied when
    /// time ran out.
    Draw { time_up: bool },
}

/// Where one player finished a round.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Placement {
    pub who: Entrant,
    /// 1 is first. Players who tie share a place, and the next place is
    /// skipped: 1, 2, 2, 4.
    pub place: usize,
    pub length: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoundEvent {
    Ended {
        outcome: Outcome,
        placements: Vec<Placement>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum JoinError {
    Full,
    /// Matched rooms only take the players they were made for.
    Closed,
}

/// Times are on the round's own clock.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Waiting {
        start_at: Option<Duration>,
    },
    Countdown {
        go_at: Duration,
    },
    Running {
        ends_at: Duration,
    },
    Results {
        outcome: Outcome,
        next_at: Option<Duration>,
    },
}

struct Seat {
    who: Entrant,
    ready: bool,
    /// The snake this player steers this round.
    slot: Option<u8>,
    wins: u32,
}

/// A snake in this round, and how its player is doing. Kept when the player
/// leaves, for the results.
struct Racer {
    who: Entrant,
    length: usize,
    /// The step it died on.
    died_at: Option<u64>,
}

pub struct Round {
    pub settings: RoundSettings,
    clock: Duration,
    phase: Phase,
    seats: Vec<Seat>,
    world: Option<World>,
    /// Indexed by snake id.
    racers: Vec<Racer>,
    placements: Vec<Placement>,
    steps: u64,
    tick_accum: Duration,
    rng: Xoshiro256PlusPlus,
    changed: bool,
    shown_secs: Option<u64>,
}

impl Round {
    /// A private room, empty and waiting for players.
    pub fn new(settings: RoundSettings, seed: u64) -> Self {
        Self {
            settings,
            clock: Duration::ZERO,
            phase: Phase::Waiting { start_at: None },
            seats: Vec::new(),
            world: None,
            racers: Vec::new(),
            placements: Vec::new(),
            steps: 0,
            tick_accum: Duration::ZERO,
            rng: Xoshiro256PlusPlus::seed_from_u64(seed),
            changed: true,
            shown_secs: None,
        }
    }

    /// A matched room: these players, counting down at once.
    pub fn new_match(settings: RoundSettings, seed: u64, players: Vec<Entrant>) -> Self {
        let mut round = Self::new(settings, seed);
        for who in players.into_iter().take(round.settings.max_players) {
            round.seats.push(Seat {
                who,
                ready: false,
                slot: None,
                wins: 0,
            });
        }
        round.begin_countdown();
        round
    }

    pub fn players(&self) -> usize {
        self.seats.len()
    }

    /// Whether no person is left: bots don't keep a room open.
    pub fn is_empty(&self) -> bool {
        self.seats.iter().all(|s| is_bot(s.who.id))
    }

    /// The room's host: the person in it longest.
    fn host(&self) -> Option<&Seat> {
        self.seats.iter().find(|s| !is_bot(s.who.id))
    }

    fn hosts(&self, id: PlayerId) -> bool {
        self.host().is_some_and(|s| s.who.id == id)
    }

    pub fn contains(&self, id: PlayerId) -> bool {
        self.seat(id).is_some()
    }

    fn seat(&self, id: PlayerId) -> Option<&Seat> {
        self.seats.iter().find(|s| s.who.id == id)
    }

    fn seat_mut(&mut self, id: PlayerId) -> Option<&mut Seat> {
        self.seats.iter_mut().find(|s| s.who.id == id)
    }

    /// Whether a round is being played right now (including its countdown).
    pub fn in_play(&self) -> bool {
        matches!(self.phase, Phase::Countdown { .. } | Phase::Running { .. })
    }

    /// Takes a seat. Players who join during a round watch it and play the
    /// next one.
    pub fn join(&mut self, who: Entrant) -> Result<(), JoinError> {
        if self.contains(who.id) {
            return Ok(());
        }
        if self.settings.kind == RoomKind::Match {
            return Err(JoinError::Closed);
        }
        if self.seats.len() >= self.settings.max_players {
            // A person takes a bot's seat, between rounds.
            let bot = self.seats.iter().rposition(|s| is_bot(s.who.id));
            match bot {
                Some(i) if !self.in_play() => {
                    self.seats.remove(i);
                }
                _ => return Err(JoinError::Full),
            }
        }
        self.seats.push(Seat {
            who,
            ready: false,
            slot: None,
            wins: 0,
        });
        self.arm_start_timer();
        self.changed = true;
        Ok(())
    }

    /// Gives up a seat. A snake still in play dies on the spot.
    pub fn leave(&mut self, id: PlayerId) -> Vec<RoundEvent> {
        let Some(i) = self.seats.iter().position(|s| s.who.id == id) else {
            return Vec::new();
        };
        let seat = self.seats.remove(i);
        self.changed = true;
        let in_play = self.in_play();
        if let (Some(slot), Some(world)) = (seat.slot, self.world.as_mut())
            && in_play
            && world.remove(slot)
        {
            self.racers[slot as usize].died_at = Some(self.steps);
        }
        if let Phase::Waiting { start_at } = &mut self.phase
            && self.seats.len() < MIN_PLAYERS
        {
            *start_at = None;
        }
        let mut events = Vec::new();
        if matches!(self.phase, Phase::Running { .. }) {
            self.end_if_decided(&mut events);
        }
        events
    }

    /// Marks a player ready, or not ready again. Once everyone in a private
    /// room is ready, the countdown starts.
    pub fn toggle_ready(&mut self, id: PlayerId) {
        if !matches!(self.phase, Phase::Waiting { .. }) {
            return;
        }
        let Some(seat) = self.seat_mut(id) else {
            return;
        };
        seat.ready = !seat.ready;
        self.changed = true;
        if self.seats.len() >= MIN_PLAYERS && self.seats.iter().all(|s| s.ready || is_bot(s.who.id))
        {
            self.begin_countdown();
        }
    }

    /// The host adds a bot, in a private room's lobby with a seat to
    /// spare. Says whether one joined.
    pub fn add_bot(&mut self, id: PlayerId) -> bool {
        if !self.lobby_by(id) || self.seats.len() >= self.settings.max_players {
            return false;
        }
        // The first name no bot here has.
        let Some(n) = (0..BOT_NAMES.len()).find(|&n| !self.contains(bot_entrant(n).id)) else {
            return false;
        };
        self.seats.push(Seat {
            who: bot_entrant(n),
            ready: false,
            slot: None,
            wins: 0,
        });
        self.lobby_changed();
        true
    }

    /// The host takes out the bot that joined last. Says whether there was
    /// one.
    pub fn remove_bot(&mut self, id: PlayerId) -> bool {
        if !self.lobby_by(id) {
            return false;
        }
        let Some(i) = self.seats.iter().rposition(|s| is_bot(s.who.id)) else {
            return false;
        };
        self.seats.remove(i);
        self.lobby_changed();
        true
    }

    /// Whether `id` hosts this private room, and it's between rounds.
    fn lobby_by(&self, id: PlayerId) -> bool {
        self.settings.kind == RoomKind::Private
            && self.hosts(id)
            && matches!(self.phase, Phase::Waiting { .. })
    }

    /// Who plays, or by what rules, just changed: nobody is ready any more,
    /// and the start timer starts again, so nobody starts a round they
    /// didn't see.
    fn lobby_changed(&mut self) {
        let enough = self.seats.len() >= MIN_PLAYERS;
        let restart = self.clock + self.settings.start_after;
        if let Phase::Waiting { start_at } = &mut self.phase {
            *start_at = enough.then_some(restart);
        }
        for seat in &mut self.seats {
            seat.ready = false;
        }
        self.changed = true;
    }

    /// The room's host (whoever has been in it longest) changes the rules
    /// of its next rounds: in a private room's lobby only, to rules the
    /// settings menu could set, on a board with room for everyone already
    /// in. Everyone is ready no longer and the start timer starts again, so
    /// nobody plays by rules they didn't see. Says whether the rules
    /// changed.
    pub fn set_rules(&mut self, id: PlayerId, rules: Rules) -> bool {
        if !self.lobby_by(id) || !rules_in_range(&rules) {
            return false;
        }
        let Some(next) = RoundSettings::new(rules, self.settings.kind) else {
            return false;
        };
        if next.rules == self.settings.rules || next.max_players < self.seats.len() {
            return false;
        }
        self.settings.rules = next.rules;
        self.settings.max_players = next.max_players;
        self.lobby_changed();
        true
    }

    /// Queues a turn for this player's snake. Turns made during the
    /// countdown are taken on the first ticks.
    pub fn steer(&mut self, id: PlayerId, dir: Direction) {
        if !self.in_play() {
            return;
        }
        let Some(slot) = self.seat(id).and_then(|s| s.slot) else {
            return;
        };
        if let Some(world) = self.world.as_mut() {
            world.steer(slot, dir);
        }
    }

    /// Moves the round's clock forward by `dt`. A step that's overdue by more
    /// than a whole tick is skipped rather than played late, so a stalled
    /// room never makes snakes jump.
    pub fn advance(&mut self, dt: Duration) -> Vec<RoundEvent> {
        let mut events = Vec::new();
        self.clock += dt;
        match self.phase.clone() {
            Phase::Waiting { start_at: Some(at) } if self.clock >= at => self.begin_countdown(),
            Phase::Countdown { go_at } if self.clock >= go_at => {
                self.phase = Phase::Running {
                    ends_at: self.clock + self.settings.time_limit,
                };
                self.tick_accum = Duration::ZERO;
                self.changed = true;
                // Someone may have left during the countdown.
                self.end_if_decided(&mut events);
            }
            Phase::Running { ends_at } => {
                self.run(dt, &mut events);
                if matches!(self.phase, Phase::Running { .. }) && self.clock >= ends_at {
                    self.end(true, true, &mut events);
                }
            }
            Phase::Results {
                next_at: Some(at), ..
            } if self.clock >= at => self.back_to_waiting(),
            _ => {}
        }
        let shown = self.shown_secs();
        if shown != self.shown_secs {
            self.shown_secs = shown;
            self.changed = true;
        }
        events
    }

    fn run(&mut self, dt: Duration, events: &mut Vec<RoundEvent>) {
        let Some(world) = self.world.as_mut() else {
            return;
        };
        world.advance_clock(dt.min(Duration::from_secs(1)));
        let interval = Duration::from_millis(self.settings.rules.tick_ms);
        self.tick_accum += dt;
        if self.tick_accum < interval {
            return;
        }
        self.tick_accum -= interval;
        if self.tick_accum >= interval {
            self.tick_accum = Duration::ZERO;
        }
        self.steps += 1;
        for (slot, racer) in self.racers.iter().enumerate() {
            if is_bot(racer.who.id)
                && racer.died_at.is_none()
                && let Some(dir) = bot::choose(world, slot as u8)
            {
                world.steer(slot as u8, dir);
            }
        }
        let outcome = world.step();
        for (id, _) in outcome.deaths {
            self.racers[id as usize].died_at = Some(self.steps);
        }
        for snake in world.snakes.iter().filter(|s| s.alive()) {
            self.racers[snake.id as usize].length = snake.len();
        }
        self.changed = true;
        self.end_if_decided(events);
    }

    /// Time until something on screen changes: the next step while a round
    /// runs, otherwise the next second of a countdown. A room can sleep this
    /// long.
    pub fn until_next(&self) -> Duration {
        const SOON: Duration = Duration::from_millis(250);
        let until = |at: Duration| at.saturating_sub(self.clock);
        match &self.phase {
            Phase::Running { ends_at } => {
                let interval = Duration::from_millis(self.settings.rules.tick_ms);
                interval
                    .saturating_sub(self.tick_accum)
                    .min(until(*ends_at))
            }
            Phase::Waiting { start_at: Some(at) }
            | Phase::Countdown { go_at: at }
            | Phase::Results {
                next_at: Some(at), ..
            } => until(*at).min(SOON),
            _ => Duration::from_secs(1),
        }
    }

    /// Whether anything players see has changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Whole seconds left on whatever is counting down.
    fn shown_secs(&self) -> Option<u64> {
        let left = |at: Duration| at.saturating_sub(self.clock).as_secs_f64().ceil() as u64;
        match &self.phase {
            Phase::Waiting { start_at: Some(at) }
            | Phase::Countdown { go_at: at }
            | Phase::Running { ends_at: at }
            | Phase::Results {
                next_at: Some(at), ..
            } => Some(left(*at)),
            _ => None,
        }
    }

    fn arm_start_timer(&mut self) {
        if let Phase::Waiting { start_at } = &mut self.phase
            && start_at.is_none()
            && self.seats.len() >= MIN_PLAYERS
        {
            *start_at = Some(self.clock + self.settings.start_after);
        }
    }

    fn begin_countdown(&mut self) {
        let n = self.seats.len();
        // Placing snakes can fail on a crowded board; another board may work.
        let world = (0..16).find_map(|_| {
            World::with_players(self.settings.rules.clone(), self.rng.next_u64(), n).ok()
        });
        let Some(world) = world else {
            self.phase = Phase::Waiting { start_at: None };
            return;
        };
        self.racers = self
            .seats
            .iter_mut()
            .enumerate()
            .map(|(i, seat)| {
                seat.slot = Some(i as u8);
                seat.ready = false;
                Racer {
                    who: seat.who.clone(),
                    length: world.snakes[i].len(),
                    died_at: None,
                }
            })
            .collect();
        self.world = Some(world);
        self.placements.clear();
        self.steps = 0;
        self.phase = Phase::Countdown {
            go_at: self.clock + self.settings.countdown,
        };
        self.changed = true;
    }

    fn end_if_decided(&mut self, events: &mut Vec<RoundEvent>) {
        let alive = self.racers.iter().filter(|r| r.died_at.is_none()).count();
        // Nobody watches bots play on: once every person's snake is out,
        // the longest bot left wins.
        let mut people = self.racers.iter().filter(|r| !is_bot(r.who.id)).peekable();
        let people_out = people.peek().is_some() && people.all(|r| r.died_at.is_some());
        if alive <= 1 {
            self.end(false, false, events);
        } else if people_out {
            self.end(false, true, events);
        }
    }

    /// Ends the round; `by_length` ranks the snakes still alive by length.
    fn end(&mut self, time_up: bool, by_length: bool, events: &mut Vec<RoundEvent>) {
        self.placements = self.rank(by_length);
        let first: Vec<&Placement> = self.placements.iter().filter(|p| p.place == 1).collect();
        let outcome = match first.as_slice() {
            [only] => Outcome::Won {
                winner: only.who.clone(),
                time_up,
            },
            _ => Outcome::Draw { time_up },
        };
        if let Outcome::Won { winner, .. } = &outcome
            && let Some(seat) = self.seat_mut(winner.id)
        {
            seat.wins += 1;
        }
        let next_at = match self.settings.kind {
            RoomKind::Private => Some(self.clock + self.settings.results_for),
            RoomKind::Match => None,
        };
        self.phase = Phase::Results {
            outcome: outcome.clone(),
            next_at,
        };
        self.changed = true;
        events.push(RoundEvent::Ended {
            outcome,
            placements: self.placements.clone(),
        });
    }

    /// Living snakes first (by length when the round was cut short), then
    /// the dead, latest death first. Equal keys share a place.
    fn rank(&self, by_length: bool) -> Vec<Placement> {
        // Higher keys finish better.
        let key = |r: &Racer| match r.died_at {
            None if by_length => (2, r.length as u64),
            None => (2, 0),
            Some(step) => (1, step),
        };
        let mut order: Vec<&Racer> = self.racers.iter().collect();
        order.sort_by_key(|r| std::cmp::Reverse(key(r)));
        let mut placements: Vec<Placement> = Vec::with_capacity(order.len());
        for (i, r) in order.iter().enumerate() {
            let place = match placements.last() {
                Some(prev) if key(order[i - 1]) == key(r) => prev.place,
                _ => i + 1,
            };
            placements.push(Placement {
                who: r.who.clone(),
                place,
                length: r.length,
            });
        }
        placements
    }

    fn back_to_waiting(&mut self) {
        self.world = None;
        self.racers.clear();
        self.placements.clear();
        for seat in &mut self.seats {
            seat.slot = None;
            seat.ready = false;
        }
        self.phase = Phase::Waiting { start_at: None };
        self.arm_start_timer();
        self.changed = true;
    }

    pub fn view(&self) -> RoomView {
        let left = |at: Duration| at.saturating_sub(self.clock);
        let phase = match &self.phase {
            Phase::Waiting { start_at } => PhaseView::Waiting {
                starts_in: start_at.map(left),
            },
            Phase::Countdown { go_at } => PhaseView::Countdown { left: left(*go_at) },
            Phase::Running { ends_at } => PhaseView::Running {
                left: left(*ends_at),
            },
            Phase::Results { outcome, next_at } => PhaseView::Results {
                outcome: outcome.clone(),
                next_in: next_at.map(left),
            },
        };
        let mut seats: Vec<SeatView> = self
            .seats
            .iter()
            .map(|s| {
                let racer = s.slot.map(|i| &self.racers[i as usize]);
                SeatView {
                    who: s.who.clone(),
                    ready: s.ready || is_bot(s.who.id),
                    slot: s.slot,
                    alive: racer.is_some_and(|r| r.died_at.is_none()),
                    length: racer.map_or(0, |r| r.length),
                    wins: s.wins,
                    left: false,
                    ping: None,
                }
            })
            .collect();
        // Players who left mid-round stay listed until it's over.
        for (i, racer) in self.racers.iter().enumerate() {
            if !self.contains(racer.who.id) {
                seats.push(SeatView {
                    who: racer.who.clone(),
                    ready: false,
                    slot: Some(i as u8),
                    alive: false,
                    length: racer.length,
                    wins: 0,
                    left: true,
                    ping: None,
                });
            }
        }
        RoomView {
            kind: self.settings.kind,
            code: None,
            rules: self.settings.rules.clone(),
            max_players: self.settings.max_players,
            phase,
            seats,
            board: self.world.as_ref().map(World::view),
            placements: self.placements.clone(),
            ratings: Vec::new(),
        }
    }
}

/// Everything a player's screen needs to draw a room.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RoomView {
    pub kind: RoomKind,
    /// Private rooms: the code others join with.
    pub code: Option<String>,
    pub rules: Rules,
    pub max_players: usize,
    pub phase: PhaseView,
    /// In the order players joined, then anyone who left this round.
    pub seats: Vec<SeatView>,
    /// From the countdown until the next round's lobby.
    pub board: Option<MatchView>,
    /// The last round's results, including players who have left.
    pub placements: Vec<Placement>,
    /// Rated rooms: how the last round moved players' ratings.
    pub ratings: Vec<RatingChange>,
}

/// A player's shown rating before and after a rated round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RatingChange {
    pub id: PlayerId,
    pub before: u32,
    pub after: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PhaseView {
    Waiting {
        starts_in: Option<Duration>,
    },
    Countdown {
        left: Duration,
    },
    Running {
        left: Duration,
    },
    Results {
        outcome: Outcome,
        /// Private rooms: the next round's lobby opens then.
        next_in: Option<Duration>,
    },
    /// The room is gone.
    Closed {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SeatView {
    pub who: Entrant,
    pub ready: bool,
    /// The snake this player has this round; `None` while watching.
    pub slot: Option<u8>,
    pub alive: bool,
    pub length: usize,
    /// Rounds won in this room.
    pub wins: u32,
    /// Left during this round; listed until it's over.
    pub left: bool,
    /// The player's round trip to the server, once measured. The server
    /// fills it in; a round knows nothing about networks.
    pub ping: Option<Duration>,
}

impl RoomView {
    /// This player's seat, unless they've left.
    pub fn seat(&self, id: PlayerId) -> Option<&SeatView> {
        self.seats.iter().find(|s| s.who.id == id && !s.left)
    }

    /// Private rooms: who sets the rules, the person in it longest.
    pub fn host(&self) -> Option<&SeatView> {
        self.seats
            .iter()
            .find(|s| !is_bot(s.who.id))
            .filter(|s| !s.left)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::config::WallMode;
    use crate::geom::Point;
    use crate::snake::Snake;

    /// The smallest board: room for two snakes and no more.
    const SMALL: (u16, u16) = (10, 8);

    fn who(id: PlayerId) -> Entrant {
        Entrant {
            id,
            name: format!("~p{id}"),
        }
    }

    fn private() -> Round {
        Round::new(
            RoundSettings::new(quick_rules(), RoomKind::Private).unwrap(),
            7,
        )
    }

    fn phase(round: &Round) -> PhaseView {
        round.view().phase
    }

    fn tick(round: &Round) -> Duration {
        Duration::from_millis(round.settings.rules.tick_ms)
    }

    /// Advances one tick at a time until `done`, collecting events.
    fn run_until(round: &mut Round, done: impl Fn(&Round) -> bool) -> Vec<RoundEvent> {
        let mut events = Vec::new();
        for _ in 0..10_000 {
            if done(round) {
                return events;
            }
            let dt = tick(round);
            events.extend(round.advance(dt));
        }
        panic!("never got there");
    }

    /// Puts snakes exactly where a test wants them, once the countdown has
    /// started.
    fn place(round: &mut Round, snakes: &[(&[(u16, u16)], Direction)]) {
        let world = round.world.as_mut().unwrap();
        for (i, (cells, dir)) in snakes.iter().enumerate() {
            let body: VecDeque<Point> = cells.iter().map(|&(x, y)| Point { x, y }).collect();
            world.snakes[i] = Snake::new(i as u8, body, *dir);
            round.racers[i].length = cells.len();
        }
        world.foods.clear();
    }

    fn running(round: &Round) -> bool {
        matches!(round.phase, Phase::Running { .. })
    }

    fn over(round: &Round) -> bool {
        matches!(round.phase, Phase::Results { .. })
    }

    #[test]
    fn multiplayer_rules_move_on_the_tick_without_boost() {
        let settings = RoundSettings::new(Rules::default(), RoomKind::Private).unwrap();
        assert_eq!(settings.rules.move_timing, MoveTiming::OnTick);
        assert_eq!(settings.rules.boost_duration_s, 0);
        assert_eq!(settings.max_players, MAX_PLAYERS);
        let tiny = Rules {
            width: 10,
            height: 10,
            ..Rules::default()
        };
        let fits = max_players_for(&tiny);
        assert!(fits.is_none_or(|n| n < MAX_PLAYERS), "{fits:?}");
    }

    #[test]
    fn a_private_room_starts_after_the_timer_or_when_everyone_is_ready() {
        let mut round = private();
        round.join(who(1)).unwrap();
        assert_eq!(phase(&round), PhaseView::Waiting { starts_in: None });
        round.join(who(2)).unwrap();
        assert_eq!(
            phase(&round),
            PhaseView::Waiting {
                starts_in: Some(Duration::from_secs(15))
            }
        );
        round.advance(Duration::from_secs(14));
        assert!(matches!(phase(&round), PhaseView::Waiting { .. }));
        round.advance(Duration::from_secs(1));
        assert!(matches!(phase(&round), PhaseView::Countdown { .. }));

        let mut ready = private();
        ready.join(who(1)).unwrap();
        ready.join(who(2)).unwrap();
        ready.toggle_ready(1);
        ready.toggle_ready(2);
        assert!(matches!(phase(&ready), PhaseView::Countdown { .. }));
        let view = ready.view();
        assert_eq!(view.seats[0].slot, Some(0));
        assert_eq!(view.seats[1].slot, Some(1));
        assert!(view.board.is_some(), "the board shows during the countdown");
    }

    /// The room's rules, with `change` made.
    fn rules_with(round: &Round, change: impl FnOnce(&mut Rules)) -> Rules {
        let mut rules = round.settings.rules.clone();
        change(&mut rules);
        rules
    }

    #[test]
    fn the_host_changes_the_rules_in_the_lobby() {
        let mut round = private();
        round.join(who(1)).unwrap();
        round.join(who(2)).unwrap();
        round.toggle_ready(2);
        round.advance(Duration::from_secs(10));
        assert_eq!(round.view().host().map(|s| s.who.id), Some(1));

        let faster = rules_with(&round, |r| r.tick_ms = 100);
        assert!(!round.set_rules(2, faster.clone()), "only the host");
        let too_fast = rules_with(&round, |r| r.tick_ms = 25);
        assert!(!round.set_rules(1, too_fast), "out of the settings' range");
        let too_wide = rules_with(&round, |r| r.width = 200);
        assert!(!round.set_rules(1, too_wide), "out of the settings' range");
        let same = round.settings.rules.clone();
        assert!(!round.set_rules(1, same), "no change");
        assert!(round.set_rules(1, faster));
        let view = round.view();
        assert_eq!(view.rules.tick_ms, 100);
        assert!(
            view.seats.iter().all(|s| !s.ready),
            "ready again, by these rules"
        );
        assert_eq!(
            view.phase,
            PhaseView::Waiting {
                starts_in: Some(Duration::from_secs(15))
            },
            "the start timer starts again"
        );

        let open = rules_with(&round, |r| {
            r.width = 40;
            r.height = 24;
            r.wall_mode = WallMode::Open;
            r.poison_interval_s = 0;
        });
        assert!(round.set_rules(1, open.clone()));
        assert_eq!(round.view().rules, open);

        // The next round plays by them.
        round.toggle_ready(1);
        round.toggle_ready(2);
        run_until(&mut round, |r| {
            matches!(phase(r), PhaseView::Running { .. })
        });
        assert_eq!(tick(&round), Duration::from_millis(100));
        let board = round.view().board.unwrap();
        assert_eq!((board.width, board.height), (40, 24));
        let slower = rules_with(&round, |r| r.tick_ms = 200);
        assert!(!round.set_rules(1, slower), "not once the round is on");
    }

    #[test]
    fn a_room_keeps_multiplayer_timing_and_room_for_its_players() {
        let mut round = private();
        for id in 1..=3 {
            round.join(who(id)).unwrap();
        }
        let solo = rules_with(&round, |r| {
            r.move_timing = MoveTiming::Instant;
            r.boost_duration_s = 5;
        });
        assert!(!round.set_rules(1, solo), "multiplayer overrides both");
        let boosted = rules_with(&round, |r| {
            r.boost_duration_s = 5;
            r.super_growth = 4;
        });
        assert!(round.set_rules(1, boosted));
        assert_eq!(round.view().rules.boost_duration_s, 0);
        assert_eq!(round.view().rules.super_growth, 4);

        // A board too small for the three of them is refused.
        let small = rules_with(&round, |r| {
            r.width = SMALL.0;
            r.height = SMALL.1;
        });
        assert_eq!(max_players_for(&small), Some(2));
        assert!(!round.set_rules(1, small.clone()));
        round.leave(3);
        assert!(round.set_rules(1, small), "two fit");
        assert_eq!(round.view().max_players, 2);
        assert_eq!(round.join(who(3)), Err(JoinError::Full));
    }

    #[test]
    fn the_host_adds_and_removes_bots_in_the_lobby() {
        let mut round = private();
        round.join(who(1)).unwrap();
        assert!(round.add_bot(1));
        assert!(!round.add_bot(2), "only the host");
        round.join(who(2)).unwrap();
        round.toggle_ready(2);
        round.advance(Duration::from_secs(10));
        assert!(round.add_bot(1));
        let view = round.view();
        let names: Vec<&str> = view.seats.iter().map(|s| s.who.name.as_str()).collect();
        assert_eq!(names, ["~p1", "Viper", "~p2", "Cobra"]);
        assert!(
            view.seats
                .iter()
                .filter(|s| is_bot(s.who.id))
                .all(|s| s.ready)
        );
        assert!(!view.seats[2].ready, "a new bot means ready again");
        assert_eq!(
            view.phase,
            PhaseView::Waiting {
                starts_in: Some(Duration::from_secs(15))
            },
            "and the start timer starts again"
        );
        assert_eq!(view.host().map(|s| s.who.id), Some(1));

        assert!(round.remove_bot(1));
        assert!(round.add_bot(1));
        let bots: Vec<String> = round
            .view()
            .seats
            .iter()
            .filter(|s| is_bot(s.who.id))
            .map(|s| s.who.name.clone())
            .collect();
        assert_eq!(bots, ["Viper", "Cobra"], "the free name again");
        while round.add_bot(1) {}
        assert_eq!(round.players(), MAX_PLAYERS);

        // A person takes a bot's seat in a full room.
        round.join(who(3)).unwrap();
        assert_eq!(round.players(), MAX_PLAYERS);
        assert!(round.contains(3));
        // When the people go, so does the room.
        for id in 1..=3 {
            round.leave(id);
        }
        assert!(round.is_empty());

        let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let mut matched = Round::new_match(settings, 1, vec![who(1), who(2)]);
        assert!(!matched.add_bot(1), "matches have who they have");
    }

    #[test]
    fn bots_play_and_the_round_ends_once_the_people_are_out() {
        let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let players = vec![who(1), bot_entrant(0), bot_entrant(1), bot_entrant(2)];
        let mut round = Round::new_match(settings, 3, players);
        // The person never steers, so they hit a wall soon; the bots don't.
        let events = run_until(&mut round, |r| {
            matches!(phase(r), PhaseView::Results { .. })
        });
        let Some(RoundEvent::Ended {
            outcome,
            placements,
        }) = events.last()
        else {
            panic!("no results: {events:?}");
        };
        // The longest bot left wins; bots as long as each other draw.
        match outcome {
            Outcome::Won { winner, time_up } => assert!(is_bot(winner.id) && !time_up),
            Outcome::Draw { time_up } => assert!(!time_up),
        }
        assert!(
            round.steps < 100,
            "ended as the person crashed: {}",
            round.steps
        );
        let last = placements.last().unwrap();
        assert_eq!(last.who.id, 1, "{placements:?}");
        let bots_first = placements.iter().take(3).all(|p| is_bot(p.who.id));
        assert!(bots_first, "{placements:?}");
    }

    #[test]
    fn the_next_host_is_whoever_has_been_in_longest() {
        let mut round = private();
        for id in 1..=3 {
            round.join(who(id)).unwrap();
        }
        round.leave(1);
        assert_eq!(round.view().host().map(|s| s.who.id), Some(2));
        assert!(round.set_rules(2, rules_with(&round, |r| r.tick_ms = 200)));
        assert!(!round.set_rules(3, rules_with(&round, |r| r.tick_ms = 75)));

        let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let mut matched = Round::new_match(settings, 1, vec![who(1), who(2)]);
        let faster = rules_with(&matched, |r| r.tick_ms = 100);
        assert!(!matched.set_rules(1, faster), "matches keep theirs");
    }

    #[test]
    fn dropping_below_two_players_stops_the_start_timer() {
        let mut round = private();
        round.join(who(1)).unwrap();
        round.join(who(2)).unwrap();
        round.leave(2);
        assert_eq!(phase(&round), PhaseView::Waiting { starts_in: None });
        round.advance(Duration::from_secs(60));
        assert_eq!(phase(&round), PhaseView::Waiting { starts_in: None });
    }

    #[test]
    fn rooms_have_a_size_limit_and_matches_take_no_one_else() {
        let mut round = private();
        for id in 0..MAX_PLAYERS as u64 {
            round.join(who(id)).unwrap();
        }
        assert_eq!(round.join(who(99)), Err(JoinError::Full));
        assert_eq!(round.join(who(0)), Ok(()), "already in");

        let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let mut matched = Round::new_match(settings, 1, vec![who(1), who(2)]);
        assert!(matches!(phase(&matched), PhaseView::Countdown { .. }));
        assert_eq!(matched.join(who(3)), Err(JoinError::Closed));
    }

    fn matched(players: u64, seed: u64) -> Round {
        let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
        Round::new_match(settings, seed, (1..=players).map(who).collect())
    }

    #[test]
    fn a_head_on_crash_is_a_draw() {
        let mut round = matched(2, 1);
        place(
            &mut round,
            &[
                (&[(10, 5), (9, 5), (8, 5)], Direction::Right),
                (&[(12, 5), (13, 5), (14, 5)], Direction::Left),
            ],
        );
        let events = run_until(&mut round, over);
        let [
            RoundEvent::Ended {
                outcome,
                placements,
            },
        ] = events.as_slice()
        else {
            panic!("{events:?}")
        };
        assert_eq!(outcome, &Outcome::Draw { time_up: false });
        assert!(placements.iter().all(|p| p.place == 1));
    }

    #[test]
    fn the_last_snake_alive_wins_and_places_follow_the_order_of_death() {
        let mut round = matched(3, 1);
        place(
            &mut round,
            &[
                // Into the left wall after 2 steps.
                (&[(1, 3), (2, 3), (3, 3)], Direction::Left),
                // Into the left wall after 3 steps.
                (&[(2, 9), (3, 9), (4, 9)], Direction::Left),
                // Free for a long while.
                (&[(10, 15), (9, 15), (8, 15)], Direction::Right),
            ],
        );
        let events = run_until(&mut round, over);
        let RoundEvent::Ended {
            outcome,
            placements,
        } = &events[0];
        assert_eq!(
            outcome,
            &Outcome::Won {
                winner: who(3),
                time_up: false
            }
        );
        let order: Vec<(PlayerId, usize)> =
            placements.iter().map(|p| (p.who.id, p.place)).collect();
        assert_eq!(order, [(3, 1), (2, 2), (1, 3)]);
        assert_eq!(round.view().seats[2].wins, 1);
    }

    #[test]
    fn dying_on_the_same_tick_shares_a_place() {
        let mut round = matched(3, 1);
        place(
            &mut round,
            &[
                (&[(1, 3), (2, 3), (3, 3)], Direction::Left),
                (&[(1, 9), (2, 9), (3, 9)], Direction::Left),
                (&[(10, 15), (9, 15), (8, 15)], Direction::Right),
            ],
        );
        run_until(&mut round, over);
        let places: Vec<usize> = round.view().placements.iter().map(|p| p.place).collect();
        assert_eq!(places, [1, 2, 2]);
    }

    #[test]
    fn when_time_runs_out_the_longest_snake_wins() {
        let no_food = Rules {
            food_interval_s: 0,
            ..quick_rules()
        };
        let mut settings = RoundSettings::new(no_food, RoomKind::Match).unwrap();
        settings.time_limit = Duration::from_secs(1);
        let mut round = Round::new_match(settings, 1, vec![who(1), who(2)]);
        // Two snakes circling forever; one is longer.
        place(
            &mut round,
            &[
                (&[(5, 5), (5, 6), (6, 6), (6, 5)], Direction::Up),
                (&[(15, 5), (15, 6), (16, 6)], Direction::Up),
            ],
        );
        let script = [
            Direction::Left,
            Direction::Down,
            Direction::Right,
            Direction::Up,
        ];
        let mut events = Vec::new();
        for i in 0.. {
            if over(&round) {
                break;
            }
            if running(&round) {
                round.steer(1, script[i % 4]);
                round.steer(2, script[i % 4]);
            }
            let dt = tick(&round);
            events.extend(round.advance(dt));
        }
        let RoundEvent::Ended { outcome, .. } = &events[0];
        assert_eq!(
            outcome,
            &Outcome::Won {
                winner: who(1),
                time_up: true
            }
        );
    }

    #[test]
    fn leaving_kills_the_snake_and_can_decide_the_round() {
        let mut round = matched(3, 2);
        run_until(&mut round, running);
        assert!(round.leave(1).is_empty(), "two still playing");
        let board = round.view().board.unwrap();
        assert!(!board.snakes[0].alive);
        assert!(board.snakes[0].body.is_empty(), "it turned into food");
        let gone = round.view().seats.into_iter().find(|s| s.left).unwrap();
        assert_eq!(
            (gone.who.id, gone.alive),
            (1, false),
            "still listed this round"
        );
        let dt = tick(&round);
        round.advance(dt);
        let events = round.leave(2);
        let RoundEvent::Ended { outcome, .. } = &events[0];
        assert_eq!(
            outcome,
            &Outcome::Won {
                winner: who(3),
                time_up: false
            }
        );
        let ranked: Vec<PlayerId> = round.view().placements.iter().map(|p| p.who.id).collect();
        assert_eq!(ranked, [3, 2, 1], "players who left are still ranked");
    }

    #[test]
    fn leaving_during_the_countdown_ends_the_round_as_it_starts() {
        let mut round = matched(2, 3);
        round.leave(1);
        let events = run_until(&mut round, over);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            phase(&round),
            PhaseView::Results {
                outcome: Outcome::Won { .. },
                next_in: None
            }
        ));
    }

    #[test]
    fn private_rooms_go_round_after_round() {
        let mut round = private();
        round.join(who(1)).unwrap();
        round.join(who(2)).unwrap();
        round.toggle_ready(1);
        round.toggle_ready(2);
        run_until(&mut round, over);
        // Someone arriving now watches, then plays the next round.
        round.join(who(3)).unwrap();
        assert_eq!(round.view().seat(3).unwrap().slot, None);
        run_until(&mut round, |r| matches!(r.phase, Phase::Waiting { .. }));
        assert!(round.view().board.is_none());
        for id in 1..=3 {
            round.toggle_ready(id);
        }
        assert!(matches!(phase(&round), PhaseView::Countdown { .. }));
        assert_eq!(round.view().seat(3).unwrap().slot, Some(2));
    }

    #[test]
    fn turns_during_the_countdown_are_kept() {
        let mut round = matched(2, 4);
        let start = round.view().board.unwrap().snakes[0].body[0];
        let dir = round.world.as_ref().unwrap().snakes[0].dir;
        let turn = match dir {
            Direction::Up | Direction::Down => Direction::Left,
            Direction::Left | Direction::Right => Direction::Up,
        };
        round.steer(1, turn);
        run_until(&mut round, |r| r.steps == 1);
        let head = round.view().board.unwrap().snakes[0].body[0];
        let expected = crate::geom::wrap_next(start, turn, 30, 18);
        assert_eq!(head, expected);
    }

    #[test]
    fn late_steps_are_skipped_not_bunched_up() {
        let mut round = matched(2, 5);
        run_until(&mut round, running);
        let tick = tick(&round);
        round.advance(tick * 5);
        assert_eq!(round.steps, 1, "one step, not five");
    }

    #[test]
    fn the_same_seed_and_turns_replay_the_same_round() {
        let play = |seed| {
            let mut round = matched(4, seed);
            let mut views = Vec::new();
            for i in 0..200u64 {
                round.steer(
                    i % 4 + 1,
                    [Direction::Up, Direction::Left][(i / 7 % 2) as usize],
                );
                round.advance(Duration::from_millis(50));
                views.push(round.view());
            }
            views
        };
        assert_eq!(play(9), play(9));
        assert_ne!(play(9), play(10));
    }

    #[test]
    fn changes_are_reported_once() {
        let mut round = private();
        assert!(round.take_changed());
        assert!(!round.take_changed());
        round.join(who(1)).unwrap();
        assert!(round.take_changed());
        round.advance(Duration::from_millis(10));
        assert!(!round.take_changed(), "nothing on screen moved");
    }
}
