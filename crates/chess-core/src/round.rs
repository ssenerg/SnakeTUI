//! Chess as rooms play it: two players, a time control, and the side the
//! host takes. A round is one game, and a private room's next round is a
//! rematch.
//!
//! The [`Game`](game::Game) inside knows its players by their slots in the
//! room, 0 and 1, rather than by id, so a player who reconnects as someone
//! new takes their side back.
//!
//! A bot in the room is the computer: the round moves for it, after a
//! while on its clock, with the [`engine`](crate::engine).

use std::time::Duration;

use game_api::{Entrant, Game, GameId, Placement, RoomRng, Round, RoundEnd, is_bot};
use rand::{Rng, RngExt, SeedableRng};

use crate::control::TimeControl;
use crate::engine;
use crate::game::{self, Players};
use crate::moves::Move;
use crate::outcome::Outcome;
use crate::piece::Color;
use crate::view::GameView;

/// The side a private room's host plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Side {
    /// Drawn for each game.
    #[default]
    Random,
    White,
    Black,
}

impl Side {
    pub const ALL: [Side; 3] = [Side::Random, Side::White, Side::Black];

    pub fn name(self) -> &'static str {
        match self {
            Side::Random => "random",
            Side::White => "white",
            Side::Black => "black",
        }
    }
}

pub use crate::engine::LEVELS;

/// What a room plays by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChessSettings {
    pub control: TimeControl,
    /// The host's side; matched rooms draw sides.
    pub side: Side,
    /// The computer's level, if the computer plays: one of [`LEVELS`].
    pub level: u8,
}

impl Default for ChessSettings {
    fn default() -> Self {
        Self {
            control: TimeControl::minutes(10, 0),
            side: Side::Random,
            level: 3,
        }
    }
}

/// What a player does in a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ChessInput {
    Move(Move),
    /// Offers a draw, or accepts the other side's standing offer.
    OfferDraw,
    AcceptDraw,
    DeclineDraw,
    Resign,
    /// Calls the game off, until both sides have made a first move.
    Abort,
}

/// Chess, as rooms play it. Its one bot is the computer.
pub struct Chess;

impl Game for Chess {
    const ID: GameId = GameId::Chess;
    const BOT_NAMES: &'static [&'static str] = &["Computer"];
    const REJOIN: bool = true;

    type Settings = ChessSettings;
    type Input = ChessInput;
    /// The game, from its countdown until the next round's lobby.
    type View = Option<GameView>;
    type Round = ChessRound;

    fn allowed(settings: &ChessSettings) -> bool {
        settings.control.is_playable() && LEVELS.contains(&settings.level)
    }

    fn seats(_: &ChessSettings) -> Option<usize> {
        Some(2)
    }
}

/// The least and the most the computer takes over a move.
const THINK: (Duration, Duration) = (Duration::from_millis(300), Duration::from_secs(5));

/// The most it takes over a first move, while no clock runs.
const FIRST_THINK: Duration = Duration::from_secs(2);

/// The computer, in a game it plays.
struct Bot {
    color: Color,
    level: u8,
    /// Its own, drawn from the room's at the start, so a room's seed plays
    /// the same games.
    rng: RoomRng,
    /// When it moves, on the game's clock, while it's its turn.
    move_at: Option<Duration>,
}

impl Bot {
    /// How long it takes over its move at `t`: about a fortieth of the
    /// time it has left and half its increment, more or less.
    fn think(&mut self, game: &game::Game, t: Duration) -> Duration {
        let clock = game.clock();
        let left = clock.remaining(self.color, t);
        let about = left / 40 + clock.control().increment / 2;
        let most = if clock.running().is_some() {
            THINK.1
        } else {
            FIRST_THINK
        };
        let think = about.mul_f64(self.rng.random_range(0.5..1.5));
        think.clamp(THINK.0, most).min(left / 2)
    }
}

pub struct ChessRound {
    game: game::Game,
    /// The players, by slot, for the results.
    players: Vec<Entrant>,
    /// The computer, if it plays.
    bot: Option<Bot>,
    /// When play started, on the room's clock: the game's own clock
    /// counts from then.
    began: Option<Duration>,
    /// The seconds the view last showed counting down, and whose.
    shown: (Option<Color>, u64),
    changed: bool,
}

impl ChessRound {
    /// How long the game has been on at the room's `at`.
    fn since_start(&self, at: Duration) -> Duration {
        self.began
            .map_or(Duration::ZERO, |began| at.saturating_sub(began))
    }

    /// The side `slot` plays.
    fn color(&self, slot: u8) -> Option<Color> {
        self.game.players().color_of(u64::from(slot))
    }

    /// What's counting down at `t`, and from when: the running clock, or
    /// the side to move's first-move timer.
    fn counting(&self, t: Duration) -> Option<(Option<Color>, Duration)> {
        let clock = self.game.clock();
        if let Some(color) = clock.running() {
            return Some((Some(color), clock.remaining(color, t)));
        }
        let deadline = clock.first_move_deadline()?;
        Some((None, deadline.saturating_sub(t)))
    }

    /// The seconds shown counting down at `t`, and whose clock they're on.
    fn shown_at(&self, t: Duration) -> (Option<Color>, u64) {
        match self.counting(t) {
            Some((color, left)) => (color, whole_secs(left)),
            None => (None, 0),
        }
    }

    /// Has the computer move a while after its turn comes at `t`.
    fn schedule(&mut self, t: Duration) {
        let Some(bot) = self.bot.as_mut() else {
            return;
        };
        let turn = self.game.outcome().is_none() && self.game.side_to_move() == bot.color;
        if !turn {
            bot.move_at = None;
        } else if bot.move_at.is_none() {
            bot.move_at = Some(t + bot.think(&self.game, t));
        }
    }

    /// The computer moves, if its time has come by `t`.
    fn bot_turn(&mut self, t: Duration) {
        let Some(bot) = self.bot.as_mut() else {
            return;
        };
        if bot.move_at.is_none_or(|at| at > t) {
            return;
        }
        bot.move_at = None;
        let color = bot.color;
        let Some(mv) = engine::choose_in(&self.game, bot.level, &mut bot.rng) else {
            return;
        };
        if self.game.play(color, mv, t).is_ok() || self.game.outcome().is_some() {
            self.changed = true;
            self.shown = self.shown_at(t);
        }
    }

    /// The computer turns down a draw offered to it, at once.
    fn decline_for_bot(&mut self) {
        if let Some(bot) = &self.bot
            && self.game.draw_offer() == Some(!bot.color)
        {
            let _ = self.game.decline_draw(bot.color);
            self.changed = true;
        }
    }

    /// How the game ended, for the room, once it has.
    fn ended(&mut self) -> Option<RoundEnd> {
        let outcome = self.game.outcome()?;
        self.changed = true;
        Some(self.results(outcome))
    }

    fn results(&self, outcome: Outcome) -> RoundEnd {
        if !outcome.is_rated() {
            return RoundEnd::Aborted;
        }
        let players = self.game.players();
        let mut placements: Vec<Placement> = Color::BOTH
            .into_iter()
            .map(|color| Placement {
                who: self.players[players.get(color) as usize].clone(),
                place: match outcome.winner {
                    Some(winner) if winner != color => 2,
                    _ => 1,
                },
            })
            .collect();
        placements.sort_by_key(|p| p.place);
        RoundEnd::Ranked(placements)
    }
}

/// Whole seconds, counting a part of one as one, as clocks show them.
fn whole_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

impl Round<Chess> for ChessRound {
    fn start(settings: &ChessSettings, players: &[Entrant], rng: &mut RoomRng) -> Option<Self> {
        let [_, _] = players else {
            return None;
        };
        // Slot 0 is the host: the first to sit down.
        let host_white = match settings.side {
            Side::White => true,
            Side::Black => false,
            Side::Random => rng.random(),
        };
        let (white, black) = if host_white { (0, 1) } else { (1, 0) };
        let game = game::Game::new(settings.control, Players { white, black });
        let bot = players.iter().position(|p| is_bot(p.id)).map(|slot| Bot {
            color: if slot == white as usize {
                Color::White
            } else {
                Color::Black
            },
            level: settings.level,
            rng: RoomRng::seed_from_u64(rng.next_u64()),
            move_at: None,
        });
        let mut round = Self {
            game,
            players: players.to_vec(),
            bot,
            began: None,
            shown: (None, 0),
            changed: true,
        };
        round.shown = round.shown_at(Duration::ZERO);
        Some(round)
    }

    fn go(&mut self, at: Duration) -> Option<RoundEnd> {
        self.began = Some(at);
        self.changed = true;
        self.schedule(Duration::ZERO);
        None
    }

    fn advance(&mut self, at: Duration, _: Duration) -> Option<RoundEnd> {
        let t = self.since_start(at);
        if self.game.check_time(t).is_some() {
            return self.ended();
        }
        self.bot_turn(t);
        if self.game.outcome().is_some() {
            return self.ended();
        }
        self.schedule(t);
        let shown = self.shown_at(t);
        if shown != self.shown {
            self.shown = shown;
            self.changed = true;
        }
        None
    }

    fn input(
        &mut self,
        slot: u8,
        input: ChessInput,
        at: Duration,
        rtt: Option<Duration>,
    ) -> Option<RoundEnd> {
        // Nothing counts during the countdown.
        self.began?;
        let color = self.color(slot)?;
        if let Some(rtt) = rtt {
            self.game.set_rtt(color, rtt);
        }
        let t = self.since_start(at);
        let done = match input {
            ChessInput::Move(mv) => self.game.play(color, mv, t),
            ChessInput::OfferDraw => self.game.offer_draw(color, t),
            ChessInput::AcceptDraw => self.game.accept_draw(color, t),
            ChessInput::DeclineDraw => self.game.decline_draw(color),
            ChessInput::Resign => self.game.resign(color, t),
            ChessInput::Abort => self.game.abort(t),
        };
        // A move after a flag fell finds the game lost on time.
        if done.is_ok() || self.game.outcome().is_some() {
            self.changed = true;
            self.shown = self.shown_at(t);
        }
        self.decline_for_bot();
        self.schedule(t);
        self.ended()
    }

    fn leave(&mut self, slot: u8, at: Duration) -> Option<RoundEnd> {
        let color = self.color(slot)?;
        if self.began.is_none() {
            // Gone before the first move could be made.
            self.changed = true;
            return Some(RoundEnd::Aborted);
        }
        let _ = self.game.abandon(color, self.since_start(at));
        self.ended()
    }

    fn until_next(&self, at: Duration) -> Duration {
        let t = self.since_start(at);
        // The next second shown, then whatever may run out.
        let tick = self.counting(t).and_then(|(_, left)| {
            (!left.is_zero())
                .then(|| left.saturating_sub(Duration::from_secs(whole_secs(left) - 1)))
        });
        // And the computer's move.
        let bot = self.bot.as_ref().and_then(|b| b.move_at);
        let bot = bot.map(|at| at.saturating_sub(t));
        [tick, self.game.until_next(t), bot]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(Duration::from_secs(1))
    }

    fn view(&self, at: Duration) -> Option<GameView> {
        Some(self.game.view(self.since_start(at)))
    }

    fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}

#[cfg(test)]
mod tests {
    use game_api::{Room, RoomKind, RoomSettings};
    use rand::SeedableRng;

    use super::*;
    use crate::outcome::Termination;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn players() -> Vec<Entrant> {
        [1, 2]
            .map(|id| Entrant {
                id,
                name: format!("p{id}"),
            })
            .to_vec()
    }

    fn round(side: Side) -> ChessRound {
        let settings = ChessSettings {
            control: TimeControl::minutes(1, 0),
            side,
            ..ChessSettings::default()
        };
        let mut rng = RoomRng::seed_from_u64(1);
        let mut round = ChessRound::start(&settings, &players(), &mut rng).unwrap();
        round.go(secs(3));
        round
    }

    fn mv(text: &str) -> ChessInput {
        ChessInput::Move(Move::parse_uci(text).unwrap())
    }

    #[test]
    fn the_host_plays_the_side_they_chose() {
        let white = round(Side::White).view(secs(3)).unwrap();
        assert_eq!(white.players, Players { white: 0, black: 1 });
        let black = round(Side::Black).view(secs(3)).unwrap();
        assert_eq!(black.players, Players { white: 1, black: 0 });
    }

    #[test]
    fn moves_count_only_once_play_starts() {
        let settings = ChessSettings::default();
        let mut rng = RoomRng::seed_from_u64(1);
        let mut round = ChessRound::start(&settings, &players(), &mut rng).unwrap();
        let white = round.game.players().white as u8;
        assert_eq!(round.input(white, mv("e2e4"), secs(1), None), None);
        assert!(
            round.view(secs(1)).unwrap().moves.is_empty(),
            "the countdown"
        );
        round.go(secs(3));
        round.input(white, mv("e2e4"), secs(4), None);
        assert_eq!(round.view(secs(4)).unwrap().moves.len(), 1);
        assert_eq!(
            round.input(white, mv("d2d4"), secs(5), None),
            None,
            "not white's turn"
        );
        assert_eq!(round.view(secs(5)).unwrap().moves.len(), 1);
    }

    #[test]
    fn mate_ranks_the_winner_first() {
        let mut round = round(Side::White);
        // Fool's mate: black, slot 1, wins.
        let moves = [(0, "f2f3"), (1, "e7e5"), (0, "g2g4")];
        for (i, (slot, text)) in moves.into_iter().enumerate() {
            assert_eq!(round.input(slot, mv(text), secs(4 + i as u64), None), None);
        }
        let Some(RoundEnd::Ranked(placements)) = round.input(1, mv("d8h4"), secs(8), None) else {
            panic!("mate ends the game");
        };
        let ranked: Vec<(u64, usize)> = placements.iter().map(|p| (p.who.id, p.place)).collect();
        assert_eq!(ranked, [(2, 1), (1, 2)]);
        let outcome = round.view(secs(8)).unwrap().outcome.unwrap();
        assert_eq!(outcome.termination, Termination::Checkmate);
    }

    #[test]
    fn a_draw_by_agreement_ranks_both_first() {
        let mut round = round(Side::White);
        round.input(0, mv("e2e4"), secs(4), None);
        round.input(1, mv("e7e5"), secs(5), None);
        assert_eq!(round.input(0, ChessInput::OfferDraw, secs(6), None), None);
        let Some(RoundEnd::Ranked(placements)) =
            round.input(1, ChessInput::AcceptDraw, secs(7), None)
        else {
            panic!("an accepted draw ends the game");
        };
        assert!(placements.iter().all(|p| p.place == 1));
    }

    #[test]
    fn time_runs_out_and_first_moves_wait_only_so_long() {
        let mut round = round(Side::White);
        assert!(matches!(
            round.advance(secs(3) + clock_first_move(), secs(30)),
            Some(RoundEnd::Aborted)
        ));

        let mut round = round_with_moves();
        // Black's clock runs from 5 s; a minute later their flag falls.
        assert_eq!(round.advance(secs(60), secs(55)), None);
        let Some(RoundEnd::Ranked(placements)) = round.advance(secs(66), secs(6)) else {
            panic!("black's flag fell");
        };
        assert_eq!(placements[0].who.id, 1, "white wins on time");
    }

    fn clock_first_move() -> Duration {
        crate::clock::FIRST_MOVE_TIME
    }

    fn round_with_moves() -> ChessRound {
        let mut round = round(Side::White);
        round.input(0, mv("e2e4"), secs(4), None);
        round.input(1, mv("e7e5"), secs(4), None);
        round.input(0, mv("g1f3"), secs(5), None);
        round
    }

    #[test]
    fn views_change_each_second_shown() {
        let mut round = round_with_moves();
        round.take_changed();
        // Black's clock shows 60 s until it drops under, a second later.
        assert_eq!(round.until_next(secs(5)), secs(1));
        assert_eq!(
            round.advance(secs(5) + Duration::from_millis(400), secs(1)),
            None
        );
        assert!(!round.take_changed(), "still 60 s");
        assert_eq!(
            round.until_next(secs(5) + Duration::from_millis(400)),
            Duration::from_millis(600)
        );
        round.advance(secs(6), secs(1));
        assert!(round.take_changed(), "59 s");
    }

    #[test]
    fn leaving_abandons_and_before_play_aborts() {
        let mut round = round_with_moves();
        let Some(RoundEnd::Ranked(placements)) = round.leave(1, secs(6)) else {
            panic!("black left");
        };
        assert_eq!(placements[0].who.id, 1);
        let outcome = round.view(secs(6)).unwrap().outcome.unwrap();
        assert_eq!(outcome.termination, Termination::Abandoned);

        let mut rng = RoomRng::seed_from_u64(1);
        let mut round = ChessRound::start(&ChessSettings::default(), &players(), &mut rng).unwrap();
        assert!(matches!(round.leave(0, secs(1)), Some(RoundEnd::Aborted)));
    }

    /// A game of `control` between p1 and the computer at `level`, p1
    /// hosting with `side`, from `seed`, under way from 3 s.
    fn against_bot(control: TimeControl, side: Side, level: u8, seed: u64) -> ChessRound {
        let settings = ChessSettings {
            control,
            side,
            level,
        };
        let players = [players()[0].clone(), Chess::bot(0)];
        let mut rng = RoomRng::seed_from_u64(seed);
        let mut round = ChessRound::start(&settings, &players, &mut rng).unwrap();
        round.go(secs(3));
        round
    }

    /// Wakes the round as a room would, from `at`, until a move more than
    /// `moves` is on the board; returns when.
    fn wake_until_moved(round: &mut ChessRound, mut at: Duration, moves: usize) -> Duration {
        for _ in 0..1000 {
            if round.view(at).unwrap().moves.len() > moves {
                return at;
            }
            at += round.until_next(at);
            assert_eq!(round.advance(at, Duration::ZERO), None);
        }
        panic!("no move");
    }

    /// The first of p1's legal moves.
    fn any_move(round: &ChessRound) -> ChessInput {
        ChessInput::Move(round.game.position().legal_moves()[0])
    }

    #[test]
    fn the_computer_thinks_then_moves_first_as_white() {
        let mut round = against_bot(TimeControl::minutes(3, 2), Side::Black, 3, 1);
        assert!(round.until_next(secs(3)) <= FIRST_THINK);
        round.advance(secs(3) + Duration::from_millis(250), Duration::ZERO);
        assert!(round.view(secs(3)).unwrap().moves.is_empty(), "too soon");
        let moved = wake_until_moved(&mut round, secs(3), 0) - secs(3);
        assert!(moved >= THINK.0 && moved <= FIRST_THINK, "{moved:?}");
        assert!(round.take_changed());
        assert_eq!(round.game.side_to_move(), Color::Black);
        assert_eq!(round.bot.as_ref().unwrap().move_at, None, "p1's turn");
    }

    #[test]
    fn the_computer_answers_and_its_clock_runs() {
        let mut round = against_bot(TimeControl::minutes(1, 0), Side::White, 1, 2);
        let mut at = secs(4);
        for ply in [0, 2, 4] {
            assert_eq!(round.input(0, any_move(&round), at, None), None);
            at = wake_until_moved(&mut round, at, ply + 1);
            at += secs(1);
        }
        let view = round.view(at).unwrap();
        assert_eq!(view.moves.len(), 6);
        assert!(view.clocks[Color::Black.index()] < secs(60), "{view:?}");
    }

    #[test]
    fn the_computer_turns_draws_down() {
        let mut round = against_bot(TimeControl::minutes(5, 0), Side::White, 2, 3);
        round.input(0, any_move(&round), secs(4), None);
        let at = wake_until_moved(&mut round, secs(4), 1);
        assert_eq!(round.input(0, ChessInput::OfferDraw, at, None), None);
        assert_eq!(round.game.draw_offer(), None);
        assert_eq!(round.game.outcome(), None);
    }

    #[test]
    fn a_seed_plays_the_same_game_again() {
        let game = |seed| {
            let mut round = against_bot(TimeControl::minutes(5, 0), Side::Random, 1, seed);
            let mut at = secs(3);
            while round.view(at).unwrap().moves.len() < 12 {
                if round.bot.as_ref().unwrap().move_at.is_none() {
                    at += secs(1);
                    round.input(0, any_move(&round), at, None);
                }
                at += round.until_next(at);
                round.advance(at, Duration::ZERO);
            }
            round.view(at).unwrap().moves
        };
        assert_eq!(game(7), game(7));
    }

    #[test]
    fn a_room_wakes_for_the_computer_until_the_game_ends() {
        for level in [1, 5] {
            let settings = ChessSettings {
                control: TimeControl::minutes(10, 0),
                side: Side::Random,
                level,
            };
            let settings = RoomSettings::<Chess>::new(settings, RoomKind::Match).unwrap();
            let me = players()[0].clone();
            let mut room = Room::<Chess>::new_match(settings, 11, vec![me.clone(), Chess::bot(0)]);
            room.advance(secs(3));
            let mut end = None;
            for _ in 0..2000 {
                let game = room.view().game.unwrap();
                if game.moves.len() >= 120 {
                    break;
                }
                let mine =
                    game.players.color_of(0) == Some(game.replay().unwrap().0.side_to_move());
                end = if mine {
                    let (position, _) = game.replay().unwrap();
                    let moves = position.legal_moves();
                    let mv = moves[game.moves.len() * 7 % moves.len()];
                    room.advance(secs(1));
                    room.play(me.id, ChessInput::Move(mv), None)
                } else {
                    room.advance(room.until_next())
                };
                if end.is_some() {
                    break;
                }
            }
            let game = room.view().game.unwrap();
            assert!(
                end.is_some() || game.moves.len() >= 120,
                "level {level}: {game:?}"
            );
        }
    }

    #[test]
    fn rooms_seat_two_and_play_chess() {
        let settings =
            RoomSettings::<Chess>::new(ChessSettings::default(), RoomKind::Match).unwrap();
        assert_eq!(settings.max_players, 2);
        let mut room = Room::<Chess>::new_match(settings, 7, players());
        assert!(
            room.view().game.is_some(),
            "the board shows in the countdown"
        );
        room.advance(secs(3));
        let white = room.view().game.unwrap().players.white;
        let id = players()[white as usize].id;
        assert_eq!(room.play(id, mv("e2e4"), None), None);
        assert_eq!(room.view().game.unwrap().moves.len(), 1);
        assert!(!Chess::allowed(&ChessSettings {
            control: TimeControl::minutes(0, 5),
            side: Side::White,
            ..ChessSettings::default()
        }));
        assert!(!Chess::allowed(&ChessSettings {
            level: 6,
            ..ChessSettings::default()
        }));
    }
}
