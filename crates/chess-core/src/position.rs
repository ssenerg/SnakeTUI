//! A position on the board, over [`cozy_chess::Board`]. Moves go in and come
//! out in standard notation; cozy-chess's own, where castling is the king
//! taking its own rook, never leaves this module.

use std::fmt;

use crate::moves::Move;
use crate::piece::{Color, Piece};
use crate::square::Square;

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Where the pieces stand, whose move it is, castling rights, the en passant
/// square and the move counters: as much as a FEN says, and no history.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Position {
    board: cozy_chess::Board,
}

/// The text wasn't the FEN of a standard chess position: it didn't parse,
/// the position can't happen, or a castling right isn't for a king on its
/// e-file square and a rook in the corner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FenError;

impl fmt::Display for FenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not the FEN of a chess position")
    }
}

impl std::error::Error for FenError {}

/// The move isn't legal in the position it was played in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IllegalMove;

impl fmt::Display for IllegalMove {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("illegal move")
    }
}

impl std::error::Error for IllegalMove {}

impl Default for Position {
    fn default() -> Self {
        Position::start()
    }
}

impl Position {
    pub fn start() -> Position {
        Position {
            board: cozy_chess::Board::startpos(),
        }
    }

    pub fn from_fen(fen: &str) -> Result<Position, FenError> {
        let board = cozy_chess::Board::from_fen(fen.trim(), false).map_err(|_| FenError)?;
        let standard = Color::BOTH.into_iter().all(|color| {
            let color = color.to_cozy();
            let rights = board.castle_rights(color);
            let home = cozy_chess::Square::new(
                cozy_chess::File::E,
                cozy_chess::Rank::First.relative_to(color),
            );
            let none = rights.short.is_none() && rights.long.is_none();
            none || (board.king(color) == home
                && rights.short.is_none_or(|f| f == cozy_chess::File::H)
                && rights.long.is_none_or(|f| f == cozy_chess::File::A))
        });
        if standard {
            Ok(Position { board })
        } else {
            Err(FenError)
        }
    }

    pub fn fen(&self) -> String {
        self.board.to_string()
    }

    pub fn side_to_move(&self) -> Color {
        Color::from_cozy(self.board.side_to_move())
    }

    pub fn in_check(&self) -> bool {
        !self.board.checkers().is_empty()
    }

    pub fn piece_at(&self, square: Square) -> Option<(Color, Piece)> {
        let square = square.to_cozy();
        let piece = self.board.piece_on(square)?;
        let color = self.board.color_on(square)?;
        Some((Color::from_cozy(color), Piece::from_cozy(piece)))
    }

    pub fn king(&self, color: Color) -> Square {
        Square::from_cozy(self.board.king(color.to_cozy()))
    }

    /// Moves since the last capture or pawn move, counting both sides'.
    /// cozy-chess stops counting at 100, the fifty-move rule's limit.
    pub fn halfmove_clock(&self) -> u32 {
        self.board.halfmove_clock().into()
    }

    /// 1 to start with, and one more after each of black's moves.
    pub fn fullmove_number(&self) -> u32 {
        self.board.fullmove_number().into()
    }

    pub fn legal_moves(&self) -> Vec<Move> {
        let us = self.board.colors(self.board.side_to_move());
        let mut moves = Vec::with_capacity(48);
        self.board.generate_moves(|piece_moves| {
            let castles = piece_moves.piece == cozy_chess::Piece::King;
            moves.extend(piece_moves.into_iter().map(|mv| {
                if castles && us.has(mv.to) {
                    castling_from_cozy(mv)
                } else {
                    move_from_cozy(mv)
                }
            }));
            false
        });
        moves
    }

    /// Whether the side to move has any legal move at all, which is quicker
    /// to answer than listing them.
    pub fn has_legal_moves(&self) -> bool {
        self.board.generate_moves(|_| true)
    }

    pub fn is_legal(&self, mv: Move) -> bool {
        self.to_cozy(mv).is_some_and(|mv| self.board.is_legal(mv))
    }

    /// Whether `mv` takes a piece, en passant included. Says nothing about
    /// whether it's legal.
    pub fn is_capture(&self, mv: Move) -> bool {
        match self.piece_at(mv.to) {
            Some((color, _)) => color != self.side_to_move(),
            None => {
                self.piece_at(mv.from)
                    .is_some_and(|(_, p)| p == Piece::Pawn)
                    && mv.from.file() != mv.to.file()
            }
        }
    }

    pub fn play(&mut self, mv: Move) -> Result<(), IllegalMove> {
        let mv = self.to_cozy(mv).ok_or(IllegalMove)?;
        self.board.try_play(mv).map_err(|_| IllegalMove)
    }

    /// The position after `mv`, if it's legal.
    pub fn after(&self, mv: Move) -> Option<Position> {
        let mut next = self.clone();
        next.play(mv).ok()?;
        Some(next)
    }

    /// The position after `mv`, which the caller knows is legal.
    pub(crate) fn after_legal(&self, mv: Move) -> Position {
        let mut next = self.clone();
        if let Some(mv) = self.to_cozy(mv) {
            next.board.play_unchecked(mv);
        }
        next
    }

    /// Equal for positions that count as the same for threefold repetition:
    /// the same pieces on the same squares, the same side to move, the same
    /// castling rights, and an en passant capture only if one is legal.
    pub(crate) fn repetition_key(&self) -> u64 {
        repetition_key(&self.board)
    }

    pub(crate) fn cozy(&self) -> &cozy_chess::Board {
        &self.board
    }

    /// `mv` as cozy-chess writes it, with castling as the king taking its own
    /// rook. `None` for what cozy-chess would misread: a king onto its own
    /// rook isn't castling in standard notation.
    fn to_cozy(&self, mv: Move) -> Option<cozy_chess::Move> {
        if mv
            .promotion
            .is_some_and(|p| !Piece::PROMOTIONS.contains(&p))
        {
            return None;
        }
        let from = mv.from.to_cozy();
        let mut to = mv.to.to_cozy();
        if self.board.piece_on(from) == Some(cozy_chess::Piece::King) {
            if self.board.colors(self.board.side_to_move()).has(to) {
                return None;
            }
            if mv.from.file() == 4 && mv.from.file().abs_diff(mv.to.file()) == 2 {
                let rook = if mv.to.file() == 6 {
                    cozy_chess::File::H
                } else {
                    cozy_chess::File::A
                };
                to = cozy_chess::Square::new(rook, from.rank());
            }
        }
        Some(cozy_chess::Move {
            from,
            to,
            promotion: mv.promotion.map(Piece::to_cozy),
        })
    }
}

/// [`Position::repetition_key`] for a bare board, which the engine searches.
pub(crate) fn repetition_key(board: &cozy_chess::Board) -> u64 {
    let Some(file) = board.en_passant() else {
        return board.hash_without_ep();
    };
    let us = board.side_to_move();
    let target = cozy_chess::Square::new(file, cozy_chess::Rank::Sixth.relative_to(us));
    let pawns = board.colored_pieces(us, cozy_chess::Piece::Pawn);
    let capturable = board.generate_moves_for(pawns, |moves| moves.to.has(target));
    if capturable {
        board.hash()
    } else {
        board.hash_without_ep()
    }
}

fn move_from_cozy(mv: cozy_chess::Move) -> Move {
    Move {
        from: Square::from_cozy(mv.from),
        to: Square::from_cozy(mv.to),
        promotion: mv.promotion.map(Piece::from_cozy),
    }
}

/// A king taking its own rook, as the king's two-square step.
fn castling_from_cozy(mv: cozy_chess::Move) -> Move {
    let file = if mv.to.file() > mv.from.file() {
        cozy_chess::File::G
    } else {
        cozy_chess::File::C
    };
    Move::new(
        Square::from_cozy(mv.from),
        Square::from_cozy(cozy_chess::Square::new(file, mv.from.rank())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uci(text: &str) -> Move {
        Move::parse_uci(text).unwrap()
    }

    fn sq(name: &str) -> Square {
        Square::parse(name).unwrap()
    }

    fn fen(text: &str) -> Position {
        Position::from_fen(text).unwrap()
    }

    /// Leaf positions `depth` moves on, every move going through the
    /// facade's notation and back.
    fn perft(position: &Position, depth: u32) -> u64 {
        let moves = position.legal_moves();
        if depth == 1 {
            return moves.len() as u64;
        }
        moves
            .into_iter()
            .map(|mv| perft(&position.after(mv).expect("legal"), depth - 1))
            .sum()
    }

    #[test]
    fn perft_start_position() {
        let start = Position::start();
        let counts: Vec<u64> = (1..=4).map(|d| perft(&start, d)).collect();
        assert_eq!(counts, [20, 400, 8902, 197_281]);
    }

    #[test]
    fn perft_kiwipete() {
        let kiwipete = fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1");
        let counts: Vec<u64> = (1..=3).map(|d| perft(&kiwipete, d)).collect();
        assert_eq!(counts, [48, 2039, 97_862]);
    }

    #[test]
    fn perft_position_3() {
        let position = fen("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1");
        let counts: Vec<u64> = (1..=4).map(|d| perft(&position, d)).collect();
        assert_eq!(counts, [14, 191, 2812, 43_238]);
    }

    #[test]
    fn perft_position_4() {
        let white = fen("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1");
        let black = fen("r2q1rk1/pP1p2pp/Q4n2/bbp1p3/Np6/1B3NBn/pPPP1PPP/R3K2R b KQ - 0 1");
        for position in [white, black] {
            let counts: Vec<u64> = (1..=3).map(|d| perft(&position, d)).collect();
            assert_eq!(counts, [6, 264, 9467]);
        }
    }

    #[test]
    fn perft_position_5() {
        let position = fen("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8");
        let counts: Vec<u64> = (1..=3).map(|d| perft(&position, d)).collect();
        assert_eq!(counts, [44, 1486, 62_379]);
    }

    #[test]
    fn castling_is_the_kings_two_square_step() {
        let mut position = fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
        let moves = position.legal_moves();
        assert!(moves.contains(&uci("e1g1")));
        assert!(moves.contains(&uci("e1c1")));
        assert!(!moves.contains(&uci("e1h1")));
        assert!(!position.is_legal(uci("e1h1")), "not castling by its rook");
        assert!(!position.is_legal(uci("e1a1")));

        position.play(uci("e1g1")).unwrap();
        assert_eq!(
            position.piece_at(sq("g1")),
            Some((Color::White, Piece::King))
        );
        assert_eq!(
            position.piece_at(sq("f1")),
            Some((Color::White, Piece::Rook))
        );
        position.play(uci("e8c8")).unwrap();
        assert_eq!(position.fen(), "2kr3r/8/8/8/8/8/8/R4RK1 w - - 2 2");
    }

    #[test]
    fn a_king_on_e1_still_steps_one_square() {
        let mut position = fen("4k3/8/8/8/8/8/8/4K2R w K - 0 1");
        assert!(position.is_legal(uci("e1f1")));
        assert!(position.is_legal(uci("e1g1")));
        position.play(uci("e1f1")).unwrap();
        assert_eq!(position.fen(), "4k3/8/8/8/8/8/8/5K1R b - - 1 1");
    }

    #[test]
    fn promotions_must_name_a_piece_and_only_promotions_may() {
        let position = fen("8/4P3/8/8/8/8/8/k6K w - - 0 1");
        assert!(!position.is_legal(uci("e7e8")));
        assert!(position.is_legal(uci("e7e8q")));
        assert!(position.is_legal(uci("e7e8n")));
        let king = Move {
            promotion: Some(Piece::King),
            ..uci("e7e8")
        };
        assert!(!position.is_legal(king));
        assert!(!Position::start().is_legal(uci("e2e4q")));
    }

    #[test]
    fn fen_round_trips() {
        for text in [
            START_FEN,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2",
            "8/8/8/8/8/8/8/k6K b - - 37 80",
        ] {
            assert_eq!(fen(text).fen(), text);
        }
        assert_eq!(Position::start().fen(), START_FEN);
    }

    #[test]
    fn fen_rejects_what_isnt_standard_chess() {
        for text in [
            "",
            "not a fen",
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0",
            "8/8/8/8/8/8/8/K6K w - - 0 1",
            // Chess960: the king isn't on the e-file.
            "rkrnnqbb/pppppppp/8/8/8/8/PPPPPPPP/RKRNNQBB w KQkq - 0 1",
            // A castling right with no rook in the corner.
            "4k3/8/8/8/8/8/8/4K3 w K - 0 1",
        ] {
            assert_eq!(Position::from_fen(text), Err(FenError), "{text}");
        }
    }

    #[test]
    fn reports_side_to_move_check_and_counters() {
        let mut position = Position::start();
        assert_eq!(position.side_to_move(), Color::White);
        for mv in ["f2f3", "e7e5", "g2g4"] {
            position.play(uci(mv)).unwrap();
        }
        assert_eq!(position.side_to_move(), Color::Black);
        assert!(!position.in_check());
        position.play(uci("d8h4")).unwrap();
        assert!(position.in_check());
        assert!(!position.has_legal_moves());
        assert_eq!(position.fullmove_number(), 3);
        assert_eq!(position.halfmove_clock(), 1);
        assert_eq!(position.play(uci("e2e4")), Err(IllegalMove));
    }

    #[test]
    fn captures_include_en_passant() {
        let position = fen("4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 2");
        assert!(position.is_capture(uci("e5d6")));
        assert!(!position.is_capture(uci("e5e6")));
        let position = fen("4k3/8/8/3p4/4P3/8/8/4K3 w - - 0 2");
        assert!(position.is_capture(uci("e4d5")));
        assert!(!position.is_capture(uci("e1e2")));
    }

    #[test]
    fn en_passant_counts_for_repetition_only_when_it_can_be_taken() {
        // After e2e4 no black pawn can take, so it's the position without it.
        let takeable = fen("4k3/8/8/8/3pP3/8/8/4K3 b - e3 0 1");
        let gone = fen("4k3/8/8/8/3pP3/8/8/4K3 b - - 0 1");
        assert_ne!(takeable.repetition_key(), gone.repetition_key());
        let untakeable = fen("4k3/8/8/8/4P3/8/8/4K3 b - e3 0 1");
        let plain = fen("4k3/8/8/8/4P3/8/8/4K3 b - - 0 1");
        assert_eq!(untakeable.repetition_key(), plain.repetition_key());
    }
}
