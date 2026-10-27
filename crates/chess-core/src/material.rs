//! Whether a side has enough left to ever give mate, for draws by
//! insufficient material and for flags that fall against a lone king.

use cozy_chess::{BitBoard, Piece as P};

use crate::piece::Color;
use crate::position::Position;

impl Position {
    /// Whether `color` could still mate by some series of legal moves, with
    /// the other side's help if it comes to that. A lone king can't; a king
    /// and one knight can't against a king with at most queens beside it; a
    /// king and bishops can't when every bishop on the board stands on one
    /// colour of square and the other side has no knight or pawn to block
    /// with. Anything more can.
    pub fn can_mate(&self, color: Color) -> bool {
        let board = self.cozy();
        let ours = board.colors(color.to_cozy());
        let theirs = board.colors((!color).to_cozy());
        let heavy = board.pieces(P::Pawn) | board.pieces(P::Rook) | board.pieces(P::Queen);
        if !(ours & heavy).is_empty() {
            return true;
        }
        let knights = ours & board.pieces(P::Knight);
        let bishops = ours & board.pieces(P::Bishop);
        match (knights.len(), bishops.len()) {
            (0, 0) => false,
            (1, 0) => {
                let blockers = theirs & !(board.pieces(P::King) | board.pieces(P::Queen));
                !blockers.is_empty()
            }
            (0, _) => {
                let blockers = theirs & (board.pieces(P::Knight) | board.pieces(P::Pawn));
                let all = board.pieces(P::Bishop);
                let both_colours = !(all & BitBoard::LIGHT_SQUARES).is_empty()
                    && !(all & BitBoard::DARK_SQUARES).is_empty();
                !blockers.is_empty() || both_colours
            }
            _ => true,
        }
    }

    /// Whether neither side can ever mate: king against king, a king and one
    /// minor piece against a king, or kings with only bishops beside them,
    /// all on one colour of square.
    pub fn insufficient_material(&self) -> bool {
        !self.can_mate(Color::White) && !self.can_mate(Color::Black)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(fen: &str) -> Position {
        Position::from_fen(fen).unwrap()
    }

    #[test]
    fn dead_positions() {
        for fen in [
            "4k3/8/8/8/8/8/8/4K3 w - - 0 1",
            "4k3/8/8/8/8/8/8/4KN2 w - - 0 1",
            "4kb2/8/8/8/8/8/8/4K3 w - - 0 1",
            // Every bishop on a light square, however many.
            "4k3/8/8/8/8/8/8/3BKB2 w - - 0 1",
            "4k3/8/8/8/2b5/8/8/3BK3 w - - 0 1",
        ] {
            assert!(position(fen).insufficient_material(), "{fen}");
        }
    }

    #[test]
    fn positions_with_mates_left_in_them() {
        for fen in [
            "4k3/8/8/8/8/8/4P3/4K3 w - - 0 1",
            "4k3/8/8/8/8/8/8/R3K3 w - - 0 1",
            // Opposite-coloured bishops can mate, with help.
            "4kb2/8/8/8/8/8/8/3BK3 w - - 0 1",
            "4kn2/8/8/8/8/8/8/4KN2 w - - 0 1",
            "4kn2/8/8/8/8/8/8/2B1K3 w - - 0 1",
            "4k3/8/8/8/8/8/8/3NKN2 w - - 0 1",
            "4k3/8/8/8/8/8/8/2BNK3 w - - 0 1",
        ] {
            assert!(!position(fen).insufficient_material(), "{fen}");
        }
    }

    #[test]
    fn whether_one_side_can_mate() {
        // A knight can't mate a king that has only a queen to block with,
        // but can one with a rook, which can get in its own king's way.
        let queen = position("q3k3/8/8/8/8/8/8/4KN2 w - - 0 1");
        assert!(!queen.can_mate(Color::White));
        assert!(queen.can_mate(Color::Black));
        let rook = position("r3k3/8/8/8/8/8/8/4KN2 w - - 0 1");
        assert!(rook.can_mate(Color::White));

        // A bishop can mate a king that has a pawn, but not one with a rook.
        let pawn = position("4k3/7p/8/8/8/8/8/4KB2 w - - 0 1");
        assert!(pawn.can_mate(Color::White));
        let rook = position("r3k3/8/8/8/8/8/8/4KB2 w - - 0 1");
        assert!(!rook.can_mate(Color::White));

        let lone = position("4k3/8/8/8/8/8/8/Q3K3 w - - 0 1");
        assert!(!lone.can_mate(Color::Black));
        assert!(Position::start().can_mate(Color::White));
    }
}
