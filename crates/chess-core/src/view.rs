use std::time::Duration;

use crate::control::TimeControl;
use crate::game::Players;
use crate::moves::Move;
use crate::outcome::Outcome;
use crate::piece::Color;
use crate::position::Position;
use crate::san;

/// Everything needed to draw a game, detached from the
/// [`crate::game::Game`] it came from: the moves rather than the board,
/// which they rebuild.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GameView {
    pub control: TimeControl,
    pub players: Players,
    /// The starting position's FEN, unless it's the usual one.
    pub start: Option<String>,
    /// Every move so far, packed by [`Move::pack`].
    pub moves: Vec<u16>,
    /// When the view was taken, since the game started.
    pub at: Duration,
    /// Each side's time left then, white first.
    pub clocks: [Duration; 2],
    /// The side whose clock is running: none while first moves are awaited
    /// or once the game is over.
    pub running: Option<Color>,
    /// Who has offered a draw the other side hasn't answered yet.
    pub draw_offer: Option<Color>,
    pub outcome: Option<Outcome>,
    /// When the side to move must make their first move by, while first
    /// moves are awaited.
    pub first_move_deadline: Option<Duration>,
}

impl GameView {
    /// The game played through: the position now, and every move in SAN.
    /// `None` if the start or a move doesn't make sense, which a view a
    /// [`crate::game::Game`] made never has.
    pub fn replay(&self) -> Option<(Position, Vec<String>)> {
        let mut position = match &self.start {
            Some(fen) => Position::from_fen(fen).ok()?,
            None => Position::start(),
        };
        let mut sans = Vec::with_capacity(self.moves.len());
        for &bits in &self.moves {
            let mv = Move::unpack(bits)?;
            sans.push(san::format(&position, mv)?);
            position = position.after_legal(mv);
        }
        Some((position, sans))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;
    use crate::outcome::Termination;

    const PLAYERS: Players = Players { white: 1, black: 2 };

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn played(moves: &[&str]) -> Game {
        let mut game = Game::new(TimeControl::minutes(5, 3), PLAYERS);
        for (i, text) in moves.iter().enumerate() {
            let by = game.side_to_move();
            let mv = Move::parse_uci(text).unwrap();
            game.play(by, mv, secs(i as u64 + 1)).unwrap();
        }
        game
    }

    #[test]
    fn shows_clocks_turn_and_offers() {
        let mut game = played(&["e2e4", "e7e5", "g1f3"]);
        game.offer_draw(Color::White, secs(3)).unwrap();
        let view = game.view(secs(13));
        assert_eq!(view.players, PLAYERS);
        assert_eq!(view.start, None);
        assert_eq!(view.moves.len(), 3);
        assert_eq!(view.at, secs(13));
        // White's 1 s on Nf3 earned 3 s; black has thought for 10 s.
        assert_eq!(view.clocks, [secs(302), secs(290)]);
        assert_eq!(view.running, Some(Color::Black));
        assert_eq!(view.draw_offer, Some(Color::White));
        assert_eq!(view.outcome, None);
        assert_eq!(view.first_move_deadline, None);
        let (position, _) = view.replay().unwrap();
        assert_eq!(position.side_to_move(), Color::Black);
    }

    #[test]
    fn shows_the_first_move_deadline() {
        let game = played(&["e2e4"]);
        let view = game.view(secs(5));
        assert_eq!(view.running, None);
        assert_eq!(view.first_move_deadline, Some(secs(31)));
    }

    #[test]
    fn replays_the_moves_as_san() {
        let mut game = played(&["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6", "h5f7"]);
        let view = game.view(secs(8));
        assert_eq!(
            view.outcome.map(|o| o.termination),
            Some(Termination::Checkmate)
        );
        let (position, sans) = view.replay().unwrap();
        assert_eq!(&position, game.position());
        assert_eq!(sans, ["e4", "e5", "Bc4", "Nc6", "Qh5", "Nf6", "Qxf7#"]);
        assert!(game.resign(Color::Black, secs(9)).is_err());
    }

    #[test]
    fn keeps_a_starting_position_other_than_the_usual() {
        let fen = "4k3/8/8/8/8/8/4P3/4K3 b - - 0 1";
        let start = Position::from_fen(fen).unwrap();
        let mut game = Game::from_position(start, TimeControl::minutes(1, 0), PLAYERS);
        game.play(Color::Black, Move::parse_uci("e8d7").unwrap(), secs(1))
            .unwrap();
        let view = game.view(secs(1));
        assert_eq!(view.start.as_deref(), Some(fen));
        let (position, sans) = view.replay().unwrap();
        assert_eq!(&position, game.position());
        assert_eq!(sans, ["Kd7"]);
    }

    #[test]
    fn a_broken_view_doesnt_replay() {
        let mut view = played(&["e2e4"]).view(secs(1));
        view.moves.push(Move::parse_uci("e2e4").unwrap().pack());
        assert_eq!(view.replay(), None);
        view.moves = vec![u16::MAX];
        assert_eq!(view.replay(), None);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn survives_the_wire() {
        let mut game = played(&["e2e4", "e7e5", "g1f3", "b8c6"]);
        game.offer_draw(Color::Black, secs(4)).unwrap();
        game.resign(Color::White, secs(6)).unwrap();
        let view = game.view(secs(7));
        let bytes = postcard::to_stdvec(&view).unwrap();
        assert!(bytes.len() < 64, "{} bytes", bytes.len());
        assert_eq!(postcard::from_bytes::<GameView>(&bytes).unwrap(), view);
    }
}
