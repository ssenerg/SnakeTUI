//! A game between two players: the position, the moves so far, the clocks,
//! draw offers, and how it ended.
//!
//! A [`Game`] only changes when told to, and every call that could end it
//! says when it happens, as time since the game started: the clocks are
//! checked first, so a move or a resignation that arrives after a flag fell
//! finds the game already lost on time.

use std::fmt;
use std::time::Duration;

use crate::clock::{Clock, Expiry};
use crate::control::TimeControl;
use crate::moves::Move;
use crate::outcome::{Outcome, Termination};
use crate::piece::Color;
use crate::position::Position;
use crate::view::GameView;

/// Identifies a player to a game. The server hands them out; the game
/// never interprets them.
pub type PlayerId = u64;

/// Who plays which side.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Players {
    pub white: PlayerId,
    pub black: PlayerId,
}

impl Players {
    pub fn get(self, color: Color) -> PlayerId {
        match color {
            Color::White => self.white,
            Color::Black => self.black,
        }
    }

    /// The side `id` plays, if they're in the game.
    pub fn color_of(self, id: PlayerId) -> Option<Color> {
        Color::BOTH.into_iter().find(|&color| self.get(color) == id)
    }
}

/// Why a player's request was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameError {
    /// The game had ended, perhaps on time just now.
    Over,
    NotYourTurn,
    Illegal,
    /// There's no draw offer from the other side to accept or decline.
    NoDrawOffer,
    /// Both sides have made a first move, so the game can't be aborted.
    TooLateToAbort,
}

impl fmt::Display for GameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GameError::Over => "the game is over",
            GameError::NotYourTurn => "it isn't your turn",
            GameError::Illegal => "illegal move",
            GameError::NoDrawOffer => "no draw has been offered",
            GameError::TooLateToAbort => "too late to abort",
        })
    }
}

impl std::error::Error for GameError {}

#[derive(Clone, Debug)]
pub struct Game {
    start: Position,
    position: Position,
    /// Every move so far, packed by [`Move::pack`].
    moves: Vec<u16>,
    /// The repetition key of every position since the last capture or pawn
    /// move, which no earlier position can repeat; the current one last.
    seen: Vec<u64>,
    clock: Clock,
    players: Players,
    draw_offer: Option<Color>,
    outcome: Option<Outcome>,
}

impl Game {
    /// A game from the usual starting position.
    pub fn new(control: TimeControl, players: Players) -> Game {
        Game::from_position(Position::start(), control, players)
    }

    /// A game from `start`, which may already be over: mate, stalemate or
    /// a dead position.
    pub fn from_position(start: Position, control: TimeControl, players: Players) -> Game {
        let mut game = Game {
            seen: vec![start.repetition_key()],
            clock: Clock::new(control, start.side_to_move()),
            position: start.clone(),
            start,
            moves: Vec::new(),
            players,
            draw_offer: None,
            outcome: None,
        };
        game.settle(Duration::ZERO);
        game
    }

    pub fn start(&self) -> &Position {
        &self.start
    }

    pub fn position(&self) -> &Position {
        &self.position
    }

    /// Every move so far, packed by [`Move::pack`].
    pub fn moves(&self) -> &[u16] {
        &self.moves
    }

    /// The repetition key of every position since the last capture or pawn
    /// move, the current one last.
    pub(crate) fn seen(&self) -> &[u64] {
        &self.seen
    }

    pub fn last_move(&self) -> Option<Move> {
        self.moves.last().copied().and_then(Move::unpack)
    }

    pub fn players(&self) -> Players {
        self.players
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    pub fn side_to_move(&self) -> Color {
        self.position.side_to_move()
    }

    /// Who has offered a draw the other side hasn't answered yet.
    pub fn draw_offer(&self) -> Option<Color> {
        self.draw_offer
    }

    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// Records `color`'s latest measured round trip, which their lag credit
    /// comes from.
    pub fn set_rtt(&mut self, color: Color, rtt: Duration) {
        self.clock.set_rtt(color, rtt);
    }

    /// Ends the game if time ran out by `at`, and returns how it ended, if
    /// it has.
    pub fn check_time(&mut self, at: Duration) -> Option<Outcome> {
        if self.outcome.is_none()
            && let Some(expiry) = self.clock.expiry(at)
        {
            self.expire(expiry, at);
        }
        self.outcome
    }

    /// How long after `at` the game may end on time if nobody moves, so the
    /// server can check then; `None` once it's over.
    pub fn until_next(&self, at: Duration) -> Option<Duration> {
        if self.outcome.is_some() {
            return None;
        }
        self.clock.until_next(at)
    }

    /// `by` plays `mv` at `at`. A move after `by`'s flag fell is refused
    /// with [`GameError::Over`], the game lost on time.
    pub fn play(&mut self, by: Color, mv: Move, at: Duration) -> Result<(), GameError> {
        self.live(at)?;
        if by != self.position.side_to_move() {
            return Err(GameError::NotYourTurn);
        }
        let next = self.position.after(mv).ok_or(GameError::Illegal)?;
        if let Err(expiry) = self.clock.on_move(at) {
            self.expire(expiry, at);
            return Err(GameError::Over);
        }
        self.position = next;
        self.moves.push(mv.pack());
        if self.position.halfmove_clock() == 0 {
            self.seen.clear();
        }
        self.seen.push(self.position.repetition_key());
        if self.draw_offer == Some(!by) {
            self.draw_offer = None;
        }
        self.settle(at);
        Ok(())
    }

    /// `by` offers a draw, which stands until the other side moves. If the
    /// other side's own offer stands, this accepts it.
    pub fn offer_draw(&mut self, by: Color, at: Duration) -> Result<(), GameError> {
        self.live(at)?;
        if self.draw_offer == Some(!by) {
            self.end(Outcome::draw(Termination::Agreement), at);
        } else {
            self.draw_offer = Some(by);
        }
        Ok(())
    }

    /// `by` accepts the other side's draw offer.
    pub fn accept_draw(&mut self, by: Color, at: Duration) -> Result<(), GameError> {
        self.live(at)?;
        if self.draw_offer != Some(!by) {
            return Err(GameError::NoDrawOffer);
        }
        self.end(Outcome::draw(Termination::Agreement), at);
        Ok(())
    }

    /// `by` turns the other side's draw offer down.
    pub fn decline_draw(&mut self, by: Color) -> Result<(), GameError> {
        if self.outcome.is_some() {
            return Err(GameError::Over);
        }
        if self.draw_offer != Some(!by) {
            return Err(GameError::NoDrawOffer);
        }
        self.draw_offer = None;
        Ok(())
    }

    /// `by` resigns. Before both sides have made a first move there's
    /// nothing to lose yet, so the game is aborted instead.
    pub fn resign(&mut self, by: Color, at: Duration) -> Result<(), GameError> {
        self.forfeit(by, Termination::Resignation, at)
    }

    /// `by` left and didn't come back. As with [`Game::resign`], that only
    /// aborts a game that hasn't started.
    pub fn abandon(&mut self, by: Color, at: Duration) -> Result<(), GameError> {
        self.forfeit(by, Termination::Abandoned, at)
    }

    /// Calls the game off, which either side may do until both have made a
    /// first move.
    pub fn abort(&mut self, at: Duration) -> Result<(), GameError> {
        self.live(at)?;
        if self.clock.started() {
            return Err(GameError::TooLateToAbort);
        }
        self.end(Outcome::draw(Termination::Aborted), at);
        Ok(())
    }

    /// The game as of `at`, to draw or send.
    pub fn view(&self, at: Duration) -> GameView {
        GameView {
            control: self.clock.control(),
            players: self.players,
            start: (self.start != Position::start()).then(|| self.start.fen()),
            moves: self.moves.clone(),
            at,
            clocks: Color::BOTH.map(|color| self.clock.remaining(color, at)),
            running: self.clock.running(),
            draw_offer: self.draw_offer,
            outcome: self.outcome,
            first_move_deadline: self.clock.first_move_deadline(),
        }
    }

    /// Ends the game on time if it ran out by `at`; `Over` if it has ended.
    fn live(&mut self, at: Duration) -> Result<(), GameError> {
        match self.check_time(at) {
            Some(_) => Err(GameError::Over),
            None => Ok(()),
        }
    }

    fn forfeit(&mut self, by: Color, how: Termination, at: Duration) -> Result<(), GameError> {
        self.live(at)?;
        let outcome = if self.clock.started() {
            Outcome::win(how, !by)
        } else {
            Outcome::draw(Termination::Aborted)
        };
        self.end(outcome, at);
        Ok(())
    }

    fn expire(&mut self, expiry: Expiry, at: Duration) {
        let outcome = match expiry {
            Expiry::FirstMove(_) => Outcome::draw(Termination::Aborted),
            Expiry::Flag(flagged) if self.position.can_mate(!flagged) => {
                Outcome::win(Termination::Timeout, !flagged)
            }
            Expiry::Flag(_) => Outcome::draw(Termination::TimeoutVsInsufficientMaterial),
        };
        self.end(outcome, at);
    }

    /// Ends the game if the position does: mate, stalemate, a dead
    /// position, a third repetition, or fifty moves without progress.
    fn settle(&mut self, at: Duration) {
        let position = &self.position;
        let outcome = if !position.has_legal_moves() {
            if position.in_check() {
                Outcome::win(Termination::Checkmate, !position.side_to_move())
            } else {
                Outcome::draw(Termination::Stalemate)
            }
        } else if position.insufficient_material() {
            Outcome::draw(Termination::InsufficientMaterial)
        } else if self.repetitions() >= 3 {
            Outcome::draw(Termination::Threefold)
        } else if position.halfmove_clock() >= 100 {
            Outcome::draw(Termination::FiftyMoves)
        } else {
            return;
        };
        self.end(outcome, at);
    }

    /// How many times the current position has come up.
    fn repetitions(&self) -> usize {
        let Some(now) = self.seen.last() else {
            return 0;
        };
        self.seen.iter().filter(|&key| key == now).count()
    }

    fn end(&mut self, outcome: Outcome, at: Duration) {
        self.outcome = Some(outcome);
        self.draw_offer = None;
        self.clock.stop(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Color = Color::White;
    const BLACK: Color = Color::Black;
    const PLAYERS: Players = Players { white: 7, black: 9 };

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn blitz() -> Game {
        Game::new(TimeControl::minutes(3, 2), PLAYERS)
    }

    fn from(fen: &str) -> Game {
        let position = Position::from_fen(fen).unwrap();
        Game::from_position(position, TimeControl::minutes(1, 0), PLAYERS)
    }

    /// Plays UCI moves a second apart, from `at`, for whoever's turn it is.
    fn play(game: &mut Game, moves: &[&str], at: Duration) -> Duration {
        let mut at = at;
        for text in moves {
            at += secs(1);
            let by = game.side_to_move();
            game.play(by, Move::parse_uci(text).unwrap(), at)
                .unwrap_or_else(|e| panic!("{text}: {e}"));
        }
        at
    }

    fn ended(game: &Game) -> Option<(Termination, Option<Color>)> {
        game.outcome().map(|o| (o.termination, o.winner))
    }

    #[test]
    fn players_have_sides() {
        assert_eq!(PLAYERS.color_of(7), Some(WHITE));
        assert_eq!(PLAYERS.color_of(9), Some(BLACK));
        assert_eq!(PLAYERS.color_of(8), None);
        assert_eq!(PLAYERS.get(BLACK), 9);
    }

    #[test]
    fn moves_are_kept_packed_and_checked() {
        let mut game = blitz();
        let e4 = Move::parse_uci("e2e4").unwrap();
        assert_eq!(game.play(BLACK, e4, secs(1)), Err(GameError::NotYourTurn));
        let bad = Move::parse_uci("e2e5").unwrap();
        assert_eq!(game.play(WHITE, bad, secs(1)), Err(GameError::Illegal));
        game.play(WHITE, e4, secs(1)).unwrap();
        play(&mut game, &["e7e5"], secs(1));
        assert_eq!(game.moves().len(), 2);
        assert_eq!(Move::unpack(game.moves()[0]), Some(e4));
        assert_eq!(game.last_move(), Move::parse_uci("e7e5"));
        assert_eq!(game.side_to_move(), WHITE);
        assert_eq!(game.outcome(), None);
    }

    #[test]
    fn checkmate() {
        let mut game = blitz();
        play(&mut game, &["f2f3", "e7e5", "g2g4", "d8h4"], Duration::ZERO);
        assert_eq!(ended(&game), Some((Termination::Checkmate, Some(BLACK))));
        let e4 = Move::parse_uci("e2e4").unwrap();
        assert_eq!(game.play(WHITE, e4, secs(9)), Err(GameError::Over));
        assert_eq!(game.until_next(secs(9)), None);
    }

    #[test]
    fn stalemate() {
        let mut game = from("k7/8/8/2Q5/8/8/8/7K w - - 0 1");
        play(&mut game, &["c5c7"], Duration::ZERO);
        assert_eq!(ended(&game), Some((Termination::Stalemate, None)));
    }

    #[test]
    fn threefold_repetition() {
        let mut game = blitz();
        let shuffle = ["g1f3", "g8f6", "f3g1", "f6g8"];
        let at = play(&mut game, &shuffle, Duration::ZERO);
        let at = play(&mut game, &shuffle[..3], at);
        assert_eq!(game.outcome(), None, "the start has come up only twice");
        play(&mut game, &shuffle[3..], at);
        assert_eq!(ended(&game), Some((Termination::Threefold, None)));
    }

    #[test]
    fn a_pawn_move_forgets_earlier_positions() {
        let mut game = blitz();
        let shuffle = ["g1f3", "g8f6", "f3g1", "f6g8"];
        let at = play(&mut game, &shuffle, Duration::ZERO);
        let at = play(&mut game, &["e2e4", "e7e5"], at);
        let at = play(&mut game, &shuffle, at);
        assert_eq!(game.outcome(), None);
        play(&mut game, &shuffle, at);
        assert_eq!(ended(&game), Some((Termination::Threefold, None)));
    }

    #[test]
    fn fifty_moves_without_progress() {
        let mut game = from("4k3/8/8/8/8/8/8/R3K3 w - - 98 80");
        play(&mut game, &["a1a2"], Duration::ZERO);
        assert_eq!(game.outcome(), None);
        play(&mut game, &["e8d8"], secs(1));
        assert_eq!(ended(&game), Some((Termination::FiftyMoves, None)));
    }

    #[test]
    fn mate_on_the_hundredth_halfmove_is_still_mate() {
        let mut game = from("7k/8/6K1/8/8/8/8/R7 w - - 99 80");
        play(&mut game, &["a1a8"], Duration::ZERO);
        assert_eq!(ended(&game), Some((Termination::Checkmate, Some(WHITE))));
    }

    #[test]
    fn insufficient_material() {
        let mut game = from("4k3/8/8/8/8/8/3q4/4K3 w - - 0 1");
        play(&mut game, &["e1d2"], Duration::ZERO);
        assert_eq!(
            ended(&game),
            Some((Termination::InsufficientMaterial, None))
        );

        let dead = from("4k3/8/8/8/8/8/8/4KB2 w - - 0 1");
        assert_eq!(
            ended(&dead),
            Some((Termination::InsufficientMaterial, None))
        );
    }

    #[test]
    fn resignation() {
        let mut game = blitz();
        let at = play(&mut game, &["e2e4", "e7e5"], Duration::ZERO);
        game.resign(WHITE, at).unwrap();
        assert_eq!(ended(&game), Some((Termination::Resignation, Some(BLACK))));
        assert_eq!(game.resign(BLACK, at), Err(GameError::Over));
    }

    #[test]
    fn resigning_or_leaving_before_both_first_moves_aborts() {
        let mut game = blitz();
        play(&mut game, &["e2e4"], Duration::ZERO);
        game.resign(BLACK, secs(2)).unwrap();
        assert_eq!(ended(&game), Some((Termination::Aborted, None)));

        let mut game = blitz();
        game.abandon(WHITE, secs(2)).unwrap();
        assert_eq!(ended(&game), Some((Termination::Aborted, None)));
    }

    #[test]
    fn abandonment() {
        let mut game = blitz();
        let at = play(&mut game, &["e2e4", "e7e5", "g1f3"], Duration::ZERO);
        game.abandon(BLACK, at + secs(20)).unwrap();
        assert_eq!(ended(&game), Some((Termination::Abandoned, Some(WHITE))));
    }

    #[test]
    fn timeout() {
        let mut game = Game::new(TimeControl::minutes(1, 0), PLAYERS);
        let at = play(&mut game, &["e2e4", "e7e5"], Duration::ZERO);
        assert_eq!(game.until_next(at), Some(secs(60)));
        assert_eq!(game.check_time(at + secs(59)), None);
        let flag = game.check_time(at + secs(60));
        assert_eq!(flag, Some(Outcome::win(Termination::Timeout, BLACK)));
        assert_eq!(game.view(at + secs(99)).clocks, [Duration::ZERO, secs(60)]);
    }

    #[test]
    fn a_move_after_the_flag_fell_loses_on_time() {
        let mut game = Game::new(TimeControl::minutes(1, 0), PLAYERS);
        let at = play(&mut game, &["e2e4", "e7e5"], Duration::ZERO);
        let late = Move::parse_uci("g1f3").unwrap();
        assert_eq!(game.play(WHITE, late, at + secs(61)), Err(GameError::Over));
        assert_eq!(ended(&game), Some((Termination::Timeout, Some(BLACK))));

        // So does resigning, or accepting a draw.
        let mut game = Game::new(TimeControl::minutes(1, 0), PLAYERS);
        let at = play(&mut game, &["e2e4", "e7e5"], Duration::ZERO);
        game.offer_draw(BLACK, at).unwrap();
        assert_eq!(game.accept_draw(WHITE, at + secs(61)), Err(GameError::Over));
        assert_eq!(ended(&game), Some((Termination::Timeout, Some(BLACK))));
    }

    #[test]
    fn timeout_against_a_side_that_cant_mate_is_a_draw() {
        let mut game = from("4k3/8/8/8/8/8/8/R3K3 w - - 0 1");
        let at = play(&mut game, &["a1a2", "e8d8"], Duration::ZERO);
        game.check_time(at + secs(60));
        let draw = (Termination::TimeoutVsInsufficientMaterial, None);
        assert_eq!(ended(&game), Some(draw));

        let mut game = from("4k3/8/8/8/8/8/8/R3K3 w - - 0 1");
        let at = play(&mut game, &["a1a2", "e8d8", "a2a3"], Duration::ZERO);
        game.check_time(at + secs(60));
        assert_eq!(ended(&game), Some((Termination::Timeout, Some(WHITE))));
    }

    #[test]
    fn draw_by_agreement() {
        let mut game = blitz();
        let at = play(&mut game, &["e2e4", "e7e5"], Duration::ZERO);
        assert_eq!(game.accept_draw(BLACK, at), Err(GameError::NoDrawOffer));
        game.offer_draw(WHITE, at).unwrap();
        assert_eq!(game.draw_offer(), Some(WHITE));
        assert_eq!(game.accept_draw(WHITE, at), Err(GameError::NoDrawOffer));
        game.accept_draw(BLACK, at).unwrap();
        assert_eq!(ended(&game), Some((Termination::Agreement, None)));
        assert_eq!(game.draw_offer(), None);
    }

    #[test]
    fn offering_back_accepts() {
        let mut game = blitz();
        let at = play(&mut game, &["e2e4"], Duration::ZERO);
        game.offer_draw(WHITE, at).unwrap();
        game.offer_draw(BLACK, at).unwrap();
        assert_eq!(ended(&game), Some((Termination::Agreement, None)));
    }

    #[test]
    fn a_draw_offer_stands_until_the_other_side_moves() {
        let mut game = blitz();
        game.offer_draw(WHITE, Duration::ZERO).unwrap();
        let at = play(&mut game, &["e2e4"], Duration::ZERO);
        assert_eq!(
            game.draw_offer(),
            Some(WHITE),
            "the offer goes with the move"
        );
        play(&mut game, &["e7e5"], at);
        assert_eq!(game.draw_offer(), None, "moving on turns it down");

        game.offer_draw(WHITE, at).unwrap();
        game.decline_draw(BLACK).unwrap();
        assert_eq!(game.draw_offer(), None);
        assert_eq!(game.decline_draw(BLACK), Err(GameError::NoDrawOffer));
    }

    #[test]
    fn aborting() {
        let mut game = blitz();
        play(&mut game, &["e2e4"], Duration::ZERO);
        game.abort(secs(3)).unwrap();
        assert_eq!(ended(&game), Some((Termination::Aborted, None)));

        let mut game = blitz();
        let at = play(&mut game, &["e2e4", "e7e5"], Duration::ZERO);
        assert_eq!(game.abort(at), Err(GameError::TooLateToAbort));
    }

    #[test]
    fn missing_a_first_move_aborts() {
        let mut game = blitz();
        assert_eq!(game.until_next(Duration::ZERO), Some(secs(30)));
        assert_eq!(game.check_time(secs(29)), None);
        assert_eq!(
            game.check_time(secs(30)),
            Some(Outcome::draw(Termination::Aborted))
        );

        let mut game = blitz();
        play(&mut game, &["e2e4"], secs(9));
        assert_eq!(game.until_next(secs(10)), Some(secs(30)));
        assert_eq!(
            game.check_time(secs(40)).map(|o| o.termination),
            Some(Termination::Aborted)
        );
    }

    #[test]
    fn lag_credit_comes_from_each_sides_ping() {
        let mut game = Game::new(TimeControl::minutes(1, 0), PLAYERS);
        let at = play(&mut game, &["e2e4", "e7e5"], Duration::ZERO);
        game.set_rtt(WHITE, Duration::from_millis(300));
        assert_eq!(game.until_next(at), Some(Duration::from_millis(60_300)));
        let nf3 = Move::parse_uci("g1f3").unwrap();
        game.play(WHITE, nf3, at + secs(10)).unwrap();
        assert_eq!(
            game.view(at + secs(10)).clocks[0],
            Duration::from_millis(50_300)
        );
    }
}
