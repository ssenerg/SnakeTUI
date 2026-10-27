//! How a game ended, and who won.

use std::fmt;

use crate::piece::Color;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Termination {
    Checkmate,
    Resignation,
    /// A flag fell, and the other side could still mate.
    Timeout,
    /// A flag fell, but the other side couldn't mate: a draw.
    TimeoutVsInsufficientMaterial,
    Stalemate,
    /// The same position came up a third time.
    Threefold,
    /// Fifty moves each without a capture or a pawn move.
    FiftyMoves,
    /// Neither side can mate.
    InsufficientMaterial,
    /// A draw offer was accepted.
    Agreement,
    /// A player left and didn't come back.
    Abandoned,
    /// Called off before both sides had made a first move; no result.
    Aborted,
}

impl Termination {
    /// Whether one side wins by it.
    pub fn is_decisive(self) -> bool {
        matches!(
            self,
            Termination::Checkmate
                | Termination::Resignation
                | Termination::Timeout
                | Termination::Abandoned
        )
    }

    /// How a result came about, to follow "won by" or "drawn by".
    pub fn name(self) -> &'static str {
        match self {
            Termination::Checkmate => "checkmate",
            Termination::Resignation => "resignation",
            Termination::Timeout => "time",
            Termination::TimeoutVsInsufficientMaterial => "time against insufficient material",
            Termination::Stalemate => "stalemate",
            Termination::Threefold => "threefold repetition",
            Termination::FiftyMoves => "the fifty-move rule",
            Termination::InsufficientMaterial => "insufficient material",
            Termination::Agreement => "agreement",
            Termination::Abandoned => "abandonment",
            Termination::Aborted => "abort",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Outcome {
    pub termination: Termination,
    /// Who won; `None` for a draw or an abort.
    pub winner: Option<Color>,
}

impl Outcome {
    pub fn win(termination: Termination, winner: Color) -> Outcome {
        Outcome {
            termination,
            winner: Some(winner),
        }
    }

    /// A drawn game, or an aborted one.
    pub fn draw(termination: Termination) -> Outcome {
        Outcome {
            termination,
            winner: None,
        }
    }

    /// Whether ratings change: they do for every result but an abort.
    pub fn is_rated(self) -> bool {
        self.termination != Termination::Aborted
    }

    /// What `color` scored: 1 for a win, ½ for a draw and 0 for a loss, or
    /// `None` if the game was aborted.
    pub fn score(self, color: Color) -> Option<f64> {
        if !self.is_rated() {
            return None;
        }
        Some(match self.winner {
            None => 0.5,
            Some(winner) if winner == color => 1.0,
            Some(_) => 0.0,
        })
    }
}

/// "White won by checkmate", "Drawn by stalemate" or "Aborted".
impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let how = self.termination.name();
        match (self.termination, self.winner) {
            (Termination::Aborted, _) => f.write_str("Aborted"),
            (_, Some(Color::White)) => write!(f, "White won by {how}"),
            (_, Some(Color::Black)) => write!(f, "Black won by {how}"),
            (_, None) => write!(f, "Drawn by {how}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_wins_draws_and_aborts() {
        let mate = Outcome::win(Termination::Checkmate, Color::Black);
        assert_eq!(mate.score(Color::Black), Some(1.0));
        assert_eq!(mate.score(Color::White), Some(0.0));
        let draw = Outcome::draw(Termination::Stalemate);
        assert_eq!(draw.score(Color::White), Some(0.5));
        let aborted = Outcome::draw(Termination::Aborted);
        assert!(!aborted.is_rated());
        assert_eq!(aborted.score(Color::White), None);
    }

    #[test]
    fn reads_as_a_sentence() {
        let resigned = Outcome::win(Termination::Resignation, Color::White);
        assert_eq!(resigned.to_string(), "White won by resignation");
        let flag = Outcome::draw(Termination::TimeoutVsInsufficientMaterial);
        assert_eq!(
            flag.to_string(),
            "Drawn by time against insufficient material"
        );
        assert_eq!(Outcome::draw(Termination::Aborted).to_string(), "Aborted");
        assert!(Termination::Timeout.is_decisive());
        assert!(!Termination::TimeoutVsInsufficientMaterial.is_decisive());
    }
}
