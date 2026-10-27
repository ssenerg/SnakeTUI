//! Snake's multiplayer rounds: everyone plays on one board, and the last
//! snake alive wins. [`Snake`] is the game rooms play, and a [`Room`] runs
//! its lobby, countdown and results as it does for every game.
//!
//! A round only moves when its room tells it to, so tests drive its clock
//! by hand, and a seed replays the same games.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use game_api::{Game, GameId, RoomRng, RoundEnd};
use rand::Rng;

pub use game_api::{
    BOT, Entrant, JoinError, MIN_PLAYERS, PhaseView, Placement, PlayerId, RatingChange, RoomKind,
    SeatView, ShellView, is_bot,
};

use crate::bot;
use crate::config::{MoveTiming, Rules};
use crate::geom::Direction;
use crate::settings::rules_in_range;
use crate::view::MatchView;
use crate::world::World;

/// A room playing Snake.
pub type Room = game_api::Room<Snake>;
pub type RoomSettings = game_api::RoomSettings<Snake>;
pub type RoomView = game_api::RoomView<Snake>;

pub const MAX_PLAYERS: usize = 8;

/// Bots are named after snakes, one name for every seat but a person's.
const BOT_NAMES: [&str; MAX_PLAYERS - 1] = [
    "Viper", "Cobra", "Mamba", "Krait", "Taipan", "Adder", "Python",
];

/// How long a round plays at most. When it's up, the longest snake wins.
pub const TIME_LIMIT: Duration = Duration::from_secs(180);

/// The room's bot number `n`, from 0.
pub fn bot_entrant(n: usize) -> Entrant {
    Snake::bot(n)
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

/// Snake, as rooms play it: moves on the tick and no boost, with a snake
/// for every seat the board has room for.
pub struct Snake;

impl Game for Snake {
    const ID: GameId = GameId::Snake;
    const BOT_NAMES: &'static [&'static str] = &BOT_NAMES;

    type Settings = Rules;
    type Input = Direction;
    type View = RoundView;
    type Round = SnakeRound;

    fn for_rooms(rules: Rules) -> Rules {
        multiplayer(rules)
    }

    fn allowed(rules: &Rules) -> bool {
        rules_in_range(rules)
    }

    fn seats(rules: &Rules) -> Option<usize> {
        max_players_for(rules)
    }
}

/// `rules` as multiplayer plays them: moves on the tick, and no boost.
fn multiplayer(mut rules: Rules) -> Rules {
    rules.move_timing = MoveTiming::OnTick;
    rules.boost_duration_s = 0;
    rules
}

/// A board's size and walls: all that decides how many snakes fit.
type Board = (u16, u16, usize, usize);
/// Answers [`max_players_for`] has worked out.
static FITS: LazyLock<Mutex<HashMap<Board, Option<usize>>>> = LazyLock::new(Mutex::default);
/// More boards than the settings can make; past this, answers are dropped.
const FITS_KEPT: usize = 1 << 16;

/// How many snakes boards with these rules have room for, found by placing
/// them on a few boards. `None` if not even [`MIN_PLAYERS`] fit. Placing
/// them builds up to 28 boards, so each answer is kept: only the board's
/// size and walls decide it.
pub fn max_players_for(rules: &Rules) -> Option<usize> {
    let board = (
        rules.width,
        rules.height,
        rules.wall_mode.index(),
        rules.inner_walls.index(),
    );
    if let Some(&fit) = FITS.lock().unwrap().get(&board) {
        return fit;
    }
    let fit = (MIN_PLAYERS..=MAX_PLAYERS)
        .rev()
        .find(|&n| (0..4).all(|seed| World::with_players(rules.clone(), seed, n).is_ok()));
    let mut fits = FITS.lock().unwrap();
    if fits.len() >= FITS_KEPT {
        fits.clear();
    }
    fits.insert(board, fit);
    fit
}

/// What players see of a round, beside the room around it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RoundView {
    /// From the countdown until the next round's lobby.
    pub board: Option<MatchView>,
    /// Each snake by its slot, with how its player is doing: kept when they
    /// leave, for the results.
    pub racers: Vec<RacerView>,
    /// The round ended because time ran out.
    pub time_up: bool,
}

impl RoundView {
    /// This player's snake this round.
    pub fn racer(&self, id: PlayerId) -> Option<&RacerView> {
        self.racers.iter().find(|r| r.id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RacerView {
    pub id: PlayerId,
    pub alive: bool,
    pub length: usize,
}

/// A snake in this round, and how its player is doing.
struct Racer {
    who: Entrant,
    length: usize,
    /// The step it died on.
    died_at: Option<u64>,
}

/// One round on one board.
pub struct SnakeRound {
    tick: Duration,
    world: World,
    /// Indexed by snake id, which is the player's slot.
    racers: Vec<Racer>,
    steps: u64,
    tick_accum: Duration,
    /// How long play lasts once it starts; tests shorten it.
    time_limit: Duration,
    /// Once play has started.
    ends_at: Option<Duration>,
    time_up: bool,
    changed: bool,
}

impl game_api::Round<Snake> for SnakeRound {
    fn start(rules: &Rules, players: &[Entrant], rng: &mut RoomRng) -> Option<Self> {
        let n = players.len();
        // Placing snakes can fail on a crowded board; another board may work.
        let world =
            (0..16).find_map(|_| World::with_players(rules.clone(), rng.next_u64(), n).ok())?;
        let racers = players
            .iter()
            .enumerate()
            .map(|(i, who)| Racer {
                who: who.clone(),
                length: world.snakes[i].len(),
                died_at: None,
            })
            .collect();
        Some(Self {
            tick: Duration::from_millis(rules.tick_ms),
            world,
            racers,
            steps: 0,
            tick_accum: Duration::ZERO,
            time_limit: TIME_LIMIT,
            ends_at: None,
            time_up: false,
            changed: true,
        })
    }

    fn go(&mut self, at: Duration) -> Option<RoundEnd> {
        self.ends_at = Some(at + self.time_limit);
        self.tick_accum = Duration::ZERO;
        self.changed = true;
        self.decided()
    }

    /// A step that's overdue by more than a whole tick is skipped rather
    /// than played late, so a stalled room never makes snakes jump.
    fn advance(&mut self, at: Duration, dt: Duration) -> Option<RoundEnd> {
        if let Some(end) = self.run(dt) {
            return Some(end);
        }
        let time_up = self.ends_at.is_some_and(|ends| at >= ends);
        time_up.then(|| self.end(true, true))
    }

    /// Queues a turn for this player's snake. Turns made during the
    /// countdown are taken on the first ticks.
    fn input(
        &mut self,
        slot: u8,
        dir: Direction,
        _: Duration,
        _: Option<Duration>,
    ) -> Option<RoundEnd> {
        self.world.steer(slot, dir);
        None
    }

    /// A snake whose player leaves dies on the spot.
    fn leave(&mut self, slot: u8, _: Duration) -> Option<RoundEnd> {
        if self.world.remove(slot) {
            self.racers[slot as usize].died_at = Some(self.steps);
        }
        self.changed = true;
        // During the countdown, starting decides it.
        self.ends_at.and_then(|_| self.decided())
    }

    /// The next step, or the end of play if that comes first.
    fn until_next(&self, at: Duration) -> Duration {
        let next = self.tick.saturating_sub(self.tick_accum);
        self.time_left(at).map_or(next, |left| next.min(left))
    }

    fn time_left(&self, at: Duration) -> Option<Duration> {
        self.ends_at.map(|ends| ends.saturating_sub(at))
    }

    fn view(&self, _: Duration) -> RoundView {
        RoundView {
            board: Some(self.world.view()),
            racers: self
                .racers
                .iter()
                .map(|r| RacerView {
                    id: r.who.id,
                    alive: r.died_at.is_none(),
                    length: r.length,
                })
                .collect(),
            time_up: self.time_up,
        }
    }

    fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}

impl SnakeRound {
    fn run(&mut self, dt: Duration) -> Option<RoundEnd> {
        let world = &mut self.world;
        world.advance_clock(dt.min(Duration::from_secs(1)));
        self.tick_accum += dt;
        if self.tick_accum < self.tick {
            return None;
        }
        self.tick_accum -= self.tick;
        if self.tick_accum >= self.tick {
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
        self.decided()
    }

    /// Over once one snake is left, or once every person's is out: nobody
    /// watches bots play on, so the longest bot left wins.
    fn decided(&mut self) -> Option<RoundEnd> {
        let alive = self.racers.iter().filter(|r| r.died_at.is_none()).count();
        let mut people = self.racers.iter().filter(|r| !is_bot(r.who.id)).peekable();
        let people_out = people.peek().is_some() && people.all(|r| r.died_at.is_some());
        if alive <= 1 {
            Some(self.end(false, false))
        } else if people_out {
            Some(self.end(false, true))
        } else {
            None
        }
    }

    /// Ends the round; `by_length` ranks the snakes still alive by length.
    fn end(&mut self, time_up: bool, by_length: bool) -> RoundEnd {
        self.time_up = time_up;
        self.changed = true;
        RoundEnd::Ranked(self.rank(by_length))
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
            });
        }
        placements
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use game_api::winner;

    use super::*;
    use crate::config::WallMode;
    use crate::geom::Point;
    use crate::snake::Snake as Body;

    /// The smallest board: room for two snakes and no more.
    const SMALL: (u16, u16) = (10, 8);

    fn who(id: PlayerId) -> Entrant {
        Entrant {
            id,
            name: format!("~p{id}"),
        }
    }

    fn private() -> Room {
        Room::new(
            RoomSettings::new(quick_rules(), RoomKind::Private).unwrap(),
            7,
        )
    }

    fn phase(round: &Room) -> PhaseView {
        round.view().shell.phase
    }

    fn tick(round: &Room) -> Duration {
        Duration::from_millis(round.settings.game.tick_ms)
    }

    /// Advances one tick at a time until `done`, collecting events.
    fn run_until(round: &mut Room, done: impl Fn(&Room) -> bool) -> Vec<RoundEnd> {
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
    fn place(round: &mut Room, snakes: &[(&[(u16, u16)], Direction)]) {
        let round = round.round_mut().unwrap();
        for (i, (cells, dir)) in snakes.iter().enumerate() {
            let body: VecDeque<Point> = cells.iter().map(|&(x, y)| Point { x, y }).collect();
            round.world.snakes[i] = Body::new(i as u8, body, *dir);
            round.racers[i].length = cells.len();
        }
        round.world.foods.clear();
    }

    fn running(round: &Room) -> bool {
        matches!(phase(round), PhaseView::Running { .. })
    }

    fn over(round: &Room) -> bool {
        matches!(phase(round), PhaseView::Results { .. })
    }

    /// This room's round, once it has one.
    fn snakes(round: &Room) -> &SnakeRound {
        round.round().unwrap()
    }

    #[test]
    fn multiplayer_rules_move_on_the_tick_without_boost() {
        let settings = RoomSettings::new(Rules::default(), RoomKind::Private).unwrap();
        assert_eq!(settings.game.move_timing, MoveTiming::OnTick);
        assert_eq!(settings.game.boost_duration_s, 0);
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
        assert_eq!(view.shell.seats[0].slot, Some(0));
        assert_eq!(view.shell.seats[1].slot, Some(1));
        assert!(
            view.game.board.is_some(),
            "the board shows during the countdown"
        );
    }

    /// The room's rules, with `change` made.
    fn rules_with(round: &Room, change: impl FnOnce(&mut Rules)) -> Rules {
        let mut rules = round.settings.game.clone();
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
        assert_eq!(round.view().shell.host().map(|s| s.who.id), Some(1));

        let faster = rules_with(&round, |r| r.tick_ms = 100);
        assert!(!round.set_settings(2, faster.clone()), "only the host");
        let too_fast = rules_with(&round, |r| r.tick_ms = 25);
        assert!(
            !round.set_settings(1, too_fast),
            "out of the settings' range"
        );
        let too_wide = rules_with(&round, |r| r.width = 200);
        assert!(
            !round.set_settings(1, too_wide),
            "out of the settings' range"
        );
        let same = round.settings.game.clone();
        assert!(!round.set_settings(1, same), "no change");
        assert!(round.set_settings(1, faster));
        let view = round.view();
        assert_eq!(view.settings.tick_ms, 100);
        assert!(
            view.shell.seats.iter().all(|s| !s.ready),
            "ready again, by these rules"
        );
        assert_eq!(
            view.shell.phase,
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
        assert!(round.set_settings(1, open.clone()));
        assert_eq!(round.view().settings, open);

        // The next round plays by them.
        round.toggle_ready(1);
        round.toggle_ready(2);
        run_until(&mut round, |r| {
            matches!(phase(r), PhaseView::Running { .. })
        });
        assert_eq!(tick(&round), Duration::from_millis(100));
        let board = round.view().game.board.unwrap();
        assert_eq!((board.width, board.height), (40, 24));
        let slower = rules_with(&round, |r| r.tick_ms = 200);
        assert!(!round.set_settings(1, slower), "not once the round is on");
    }

    #[test]
    fn floods_of_rule_changes_cost_little() {
        use crate::config::InnerWalls;
        // The slowest board to work out: the largest, with the most walls.
        let big = Rules {
            width: 80,
            height: 40,
            inner_walls: InnerWalls::Many,
            ..quick_rules()
        };
        let mut round = Room::new(
            RoomSettings::new(big.clone(), RoomKind::Private).unwrap(),
            1,
        );
        round.join(who(1)).unwrap();
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert!(!round.set_settings(1, big.clone()), "no change");
            assert_eq!(max_players_for(&big), round.settings.max_players.into());
        }
        let took = started.elapsed();
        assert!(took < Duration::from_secs(1), "{took:?}");
        // Other rules don't change how many fit, so changing them is cheap too.
        let faster = Rules {
            tick_ms: 100,
            ..big
        };
        assert!(round.set_settings(1, faster));
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
        assert!(!round.set_settings(1, solo), "multiplayer overrides both");
        let boosted = rules_with(&round, |r| {
            r.boost_duration_s = 5;
            r.super_growth = 4;
        });
        assert!(round.set_settings(1, boosted));
        assert_eq!(round.view().settings.boost_duration_s, 0);
        assert_eq!(round.view().settings.super_growth, 4);

        // A board too small for the three of them is refused.
        let small = rules_with(&round, |r| {
            r.width = SMALL.0;
            r.height = SMALL.1;
        });
        assert_eq!(max_players_for(&small), Some(2));
        assert!(!round.set_settings(1, small.clone()));
        round.leave(3);
        assert!(round.set_settings(1, small), "two fit");
        assert_eq!(round.view().shell.max_players, 2);
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
        let names: Vec<&str> = view
            .shell
            .seats
            .iter()
            .map(|s| s.who.name.as_str())
            .collect();
        assert_eq!(names, ["~p1", "Viper", "~p2", "Cobra"]);
        assert!(
            view.shell
                .seats
                .iter()
                .filter(|s| is_bot(s.who.id))
                .all(|s| s.ready)
        );
        assert!(!view.shell.seats[2].ready, "a new bot means ready again");
        assert_eq!(
            view.shell.phase,
            PhaseView::Waiting {
                starts_in: Some(Duration::from_secs(15))
            },
            "and the start timer starts again"
        );
        assert_eq!(view.shell.host().map(|s| s.who.id), Some(1));

        assert!(round.remove_bot(1));
        assert!(round.add_bot(1));
        let bots: Vec<String> = round
            .view()
            .shell
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

        let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let mut matched = Room::new_match(settings, 1, vec![who(1), who(2)]);
        assert!(!matched.add_bot(1), "matches have who they have");
    }

    #[test]
    fn bots_play_and_the_round_ends_once_the_people_are_out() {
        let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let players = vec![who(1), bot_entrant(0), bot_entrant(1), bot_entrant(2)];
        let mut round = Room::new_match(settings, 3, players);
        // The person never steers, so they hit a wall soon; the bots don't.
        let events = run_until(&mut round, |r| {
            matches!(phase(r), PhaseView::Results { .. })
        });
        let Some(RoundEnd::Ranked(placements)) = events.last() else {
            panic!("no results: {events:?}");
        };
        // The longest bot left wins; bots as long as each other draw.
        if let Some(winner) = winner(placements) {
            assert!(is_bot(winner.id));
        }
        assert!(!round.view().game.time_up);
        let steps = snakes(&round).steps;
        assert!(steps < 100, "ended as the person crashed: {steps}");
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
        assert_eq!(round.view().shell.host().map(|s| s.who.id), Some(2));
        assert!(round.set_settings(2, rules_with(&round, |r| r.tick_ms = 200)));
        assert!(!round.set_settings(3, rules_with(&round, |r| r.tick_ms = 75)));

        let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let mut matched = Room::new_match(settings, 1, vec![who(1), who(2)]);
        let faster = rules_with(&matched, |r| r.tick_ms = 100);
        assert!(!matched.set_settings(1, faster), "matches keep theirs");
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

        let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let mut matched = Room::new_match(settings, 1, vec![who(1), who(2)]);
        assert!(matches!(phase(&matched), PhaseView::Countdown { .. }));
        assert_eq!(matched.join(who(3)), Err(JoinError::Closed));
    }

    fn matched(players: u64, seed: u64) -> Room {
        let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
        Room::new_match(settings, seed, (1..=players).map(who).collect())
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
        let [RoundEnd::Ranked(placements)] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(winner(placements), None, "a draw");
        assert!(!round.view().game.time_up);
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
        let [RoundEnd::Ranked(placements)] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(winner(placements), Some(&who(3)));
        assert!(!round.view().game.time_up);
        let order: Vec<(PlayerId, usize)> =
            placements.iter().map(|p| (p.who.id, p.place)).collect();
        assert_eq!(order, [(3, 1), (2, 2), (1, 3)]);
        assert_eq!(round.view().shell.seats[2].wins, 1);
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
        let places: Vec<usize> = round
            .view()
            .shell
            .placements
            .iter()
            .map(|p| p.place)
            .collect();
        assert_eq!(places, [1, 2, 2]);
    }

    #[test]
    fn when_time_runs_out_the_longest_snake_wins() {
        let no_food = Rules {
            food_interval_s: 0,
            ..quick_rules()
        };
        let settings = RoomSettings::new(no_food, RoomKind::Match).unwrap();
        let mut round = Room::new_match(settings, 1, vec![who(1), who(2)]);
        round.round_mut().unwrap().time_limit = Duration::from_secs(1);
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
                round.play(1, script[i % 4], None);
                round.play(2, script[i % 4], None);
            }
            let dt = tick(&round);
            events.extend(round.advance(dt));
        }
        assert_eq!(events.len(), 1);
        assert_eq!(round.view().shell.winner(), Some(&who(1)));
        assert!(round.view().game.time_up);
    }

    #[test]
    fn leaving_kills_the_snake_and_can_decide_the_round() {
        let mut round = matched(3, 2);
        run_until(&mut round, running);
        assert!(round.leave(1).is_none(), "two still playing");
        let board = round.view().game.board.unwrap();
        assert!(!board.snakes[0].alive);
        assert!(board.snakes[0].body.is_empty(), "it turned into food");
        let gone = round
            .view()
            .shell
            .seats
            .into_iter()
            .find(|s| s.left)
            .unwrap();
        assert_eq!(
            (gone.who.id, gone.slot),
            (1, Some(0)),
            "still listed this round"
        );
        assert!(!round.view().game.racers[0].alive);
        let dt = tick(&round);
        round.advance(dt);
        let end = round.leave(2).unwrap();
        assert_eq!(winner(end.placements()), Some(&who(3)));
        let ranked: Vec<PlayerId> = round
            .view()
            .shell
            .placements
            .iter()
            .map(|p| p.who.id)
            .collect();
        assert_eq!(ranked, [3, 2, 1], "players who left are still ranked");
    }

    #[test]
    fn leaving_during_the_countdown_ends_the_round_as_it_starts() {
        let mut round = matched(2, 3);
        round.leave(1);
        let events = run_until(&mut round, over);
        assert_eq!(events.len(), 1);
        assert_eq!(phase(&round), PhaseView::Results { next_in: None });
        assert!(round.view().shell.winner().is_some());
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
        assert_eq!(round.view().shell.seat(3).unwrap().slot, None);
        run_until(&mut round, |r| {
            matches!(phase(r), PhaseView::Waiting { .. })
        });
        assert!(round.view().game.board.is_none());
        for id in 1..=3 {
            round.toggle_ready(id);
        }
        assert!(matches!(phase(&round), PhaseView::Countdown { .. }));
        assert_eq!(round.view().shell.seat(3).unwrap().slot, Some(2));
    }

    #[test]
    fn turns_during_the_countdown_are_kept() {
        let mut round = matched(2, 4);
        let start = round.view().game.board.unwrap().snakes[0].body[0];
        let dir = snakes(&round).world.snakes[0].dir;
        let turn = match dir {
            Direction::Up | Direction::Down => Direction::Left,
            Direction::Left | Direction::Right => Direction::Up,
        };
        round.play(1, turn, None);
        run_until(&mut round, |r| snakes(r).steps == 1);
        let head = round.view().game.board.unwrap().snakes[0].body[0];
        let expected = crate::geom::wrap_next(start, turn, 30, 18);
        assert_eq!(head, expected);
    }

    #[test]
    fn late_steps_are_skipped_not_bunched_up() {
        let mut round = matched(2, 5);
        run_until(&mut round, running);
        let tick = tick(&round);
        round.advance(tick * 5);
        assert_eq!(snakes(&round).steps, 1, "one step, not five");
    }

    #[test]
    fn the_same_seed_and_turns_replay_the_same_round() {
        let play = |seed| {
            let mut round = matched(4, seed);
            let mut views = Vec::new();
            for i in 0..200u64 {
                round.play(
                    i % 4 + 1,
                    [Direction::Up, Direction::Left][(i / 7 % 2) as usize],
                    None,
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
