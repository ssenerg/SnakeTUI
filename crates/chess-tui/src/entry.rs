//! Entering moves: typed, in SAN or UCI and read as the player types, or
//! picked from square to square.

use chess_core::moves::Move;
use chess_core::piece::Piece;
use chess_core::position::Position;
use chess_core::san;
use chess_core::square::Square;

/// Most a typed move can be, as in `exd8=Q+`, with room to spare.
pub const MAX_TYPED: usize = 10;

/// Whether `c` can be part of a typed move.
pub fn typable(c: char) -> bool {
    c.is_ascii_alphanumeric() || "x+#=-".contains(c)
}

/// Typed text, made comparable: lower case, `0` as `o`, and without
/// captures, checks, `=` and dashes.
fn squeeze(text: &str) -> String {
    text.chars()
        .filter(|c| !"x+#=-!? ".contains(*c))
        .map(|c| {
            if c == '0' {
                'o'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// The legal moves `text` could be the start of, in SAN or UCI.
pub fn candidates(position: &Position, text: &str) -> Vec<Move> {
    let typed = squeeze(text);
    if typed.is_empty() {
        return Vec::new();
    }
    position
        .legal_moves()
        .into_iter()
        .filter(|&mv| {
            let san = san::format(position, mv).map(|s| squeeze(&s));
            mv.to_string().starts_with(&typed) || san.is_some_and(|s| s.starts_with(&typed))
        })
        .collect()
}

/// The legal moves from `from` to `to`: one, or a promotion's four.
pub fn between(position: &Position, from: Square, to: Square) -> Vec<Move> {
    position
        .legal_moves()
        .into_iter()
        .filter(|m| m.from == from && m.to == to)
        .collect()
}

/// What entering a move does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entered {
    Play(Move),
    /// A pawn reaches the last rank, and the player picks what it becomes.
    Promote {
        from: Square,
        to: Square,
    },
    /// Why nothing was played.
    Refused(&'static str),
}

/// What Enter does with `text` typed: plays the move it names, or the only
/// move it could be the start of.
pub fn enter(position: &Position, text: &str, auto_queen: bool) -> Entered {
    if let Ok(mv) = san::parse(position, text) {
        return Entered::Play(mv);
    }
    let found = candidates(position, text);
    match choose(&found, auto_queen) {
        Some(entered) => entered,
        None if found.is_empty() => Entered::Refused("That isn't a legal move here."),
        None => Entered::Refused("Which one? Type more of it."),
    }
}

/// The move `moves` leave no doubt about: the only one, or a promotion
/// that's still to be chosen, unless `auto_queen` chooses it.
pub fn choose(moves: &[Move], auto_queen: bool) -> Option<Entered> {
    let &first = moves.first()?;
    if moves.len() == 1 {
        return Some(Entered::Play(first));
    }
    let promotion = moves
        .iter()
        .all(|m| m.from == first.from && m.to == first.to && m.promotion.is_some());
    if !promotion {
        return None;
    }
    if auto_queen {
        let queen = moves.iter().find(|m| m.promotion == Some(Piece::Queen))?;
        return Some(Entered::Play(*queen));
    }
    Some(Entered::Promote {
        from: first.from,
        to: first.to,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(fen: &str) -> Position {
        Position::from_fen(fen).unwrap()
    }

    fn sq(name: &str) -> Square {
        Square::parse(name).unwrap()
    }

    #[test]
    fn typing_narrows_the_moves_down() {
        let start = Position::start();
        assert_eq!(candidates(&start, "").len(), 0);
        assert_eq!(candidates(&start, "N").len(), 4);
        let nf: Vec<String> = candidates(&start, "nf")
            .iter()
            .map(|m| m.to_string())
            .collect();
        assert_eq!(nf, ["g1f3"]);
        assert_eq!(candidates(&start, "e2").len(), 2, "UCI");
        assert_eq!(
            enter(&start, "Nf", false),
            Entered::Play(Move::parse_uci("g1f3").unwrap())
        );
        assert_eq!(
            enter(&start, "e4", false),
            Entered::Play(Move::parse_uci("e2e4").unwrap())
        );
        assert!(matches!(enter(&start, "N", false), Entered::Refused(_)));
        assert!(matches!(enter(&start, "Qh5", false), Entered::Refused(_)));
    }

    #[test]
    fn castling_reads_either_way() {
        let castle = at("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
        let short = Move::parse_uci("e1g1").unwrap();
        assert_eq!(enter(&castle, "0-0", false), Entered::Play(short));
        assert_eq!(enter(&castle, "O-O", false), Entered::Play(short));
        assert_eq!(candidates(&castle, "o-o").len(), 2, "short and long");
    }

    #[test]
    fn promotions_ask_unless_queens_are_automatic() {
        let pawn = at("8/4P3/8/8/8/8/k7/4K3 w - - 0 1");
        let promote = Entered::Promote {
            from: sq("e7"),
            to: sq("e8"),
        };
        assert_eq!(enter(&pawn, "e8", false), promote);
        assert_eq!(
            enter(&pawn, "e8", true),
            Entered::Play(Move::parse_uci("e7e8q").unwrap())
        );
        assert_eq!(
            enter(&pawn, "e8=N", false),
            Entered::Play(Move::parse_uci("e7e8n").unwrap())
        );
        assert_eq!(
            choose(&between(&pawn, sq("e7"), sq("e8")), false),
            Some(promote)
        );
        assert_eq!(between(&pawn, sq("e7"), sq("d8")), []);
    }
}
