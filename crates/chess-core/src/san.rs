//! Standard Algebraic Notation: writing moves as `Nf3`, `exd5`, `O-O` or
//! `e8=Q#`, and reading what players type, which may be SAN, UCI like
//! `e2e4`, or castling written as `0-0` or `oo`.

use std::fmt;

use crate::moves::{Move, promotion_from_letter};
use crate::piece::Piece;
use crate::position::Position;
use crate::square::Square;

/// Why typed text isn't a move.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SanError {
    /// It isn't a move in any notation we read.
    Unreadable,
    /// No legal move matches it.
    Illegal,
    /// More than one legal move matches it: `Nd2` with knights on b1 and
    /// f1, or `e8` without saying what the pawn becomes.
    Ambiguous,
}

impl fmt::Display for SanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SanError::Unreadable => "not a move",
            SanError::Illegal => "illegal move",
            SanError::Ambiguous => "ambiguous move",
        })
    }
}

impl std::error::Error for SanError {}

/// `mv` in SAN, or `None` if it isn't legal in `position`.
pub fn format(position: &Position, mv: Move) -> Option<String> {
    format_among(position, mv, &position.legal_moves())
}

/// [`format`], with `position`'s legal moves already listed.
pub(crate) fn format_among(position: &Position, mv: Move, legal: &[Move]) -> Option<String> {
    if !legal.contains(&mv) {
        return None;
    }
    let (_, piece) = position.piece_at(mv.from)?;
    let mut text = String::with_capacity(8);
    if piece == Piece::King && mv.from.file().abs_diff(mv.to.file()) == 2 {
        text.push_str(if mv.to.file() > mv.from.file() {
            "O-O"
        } else {
            "O-O-O"
        });
    } else {
        let capture = position.is_capture(mv);
        if piece == Piece::Pawn {
            if capture {
                text.push(mv.from.file_char());
            }
        } else {
            text.push(piece.letter());
            let rivals: Vec<Square> = legal
                .iter()
                .filter(|m| m.to == mv.to && m.from != mv.from)
                .filter(|m| position.piece_at(m.from).is_some_and(|(_, p)| p == piece))
                .map(|m| m.from)
                .collect();
            if !rivals.is_empty() {
                let same_file = rivals.iter().any(|s| s.file() == mv.from.file());
                let same_rank = rivals.iter().any(|s| s.rank() == mv.from.rank());
                if !same_file || same_rank {
                    text.push(mv.from.file_char());
                }
                if same_file {
                    text.push(mv.from.rank_char());
                }
            }
        }
        if capture {
            text.push('x');
        }
        text.push_str(&mv.to.to_string());
        if let Some(promotion) = mv.promotion {
            text.push('=');
            text.push(promotion.letter());
        }
    }
    let after = position.after_legal(mv);
    if after.in_check() {
        text.push(if after.has_legal_moves() { '+' } else { '#' });
    }
    Some(text)
}

/// The legal move typed text means. Reads SAN with or without `x`, `+`,
/// `#` and `=`, piece letters in either case (a lowercase `b` is the b-pawn
/// if it can make the move, else a bishop), UCI like `e2e4` or `e7e8q`,
/// long algebraic like `Ng1-f3`, and castling as `O-O`, `0-0` or `oo`.
pub fn parse(position: &Position, text: &str) -> Result<Move, SanError> {
    parse_among(position, text, &position.legal_moves())
}

/// [`parse`], with `position`'s legal moves already listed.
pub(crate) fn parse_among(
    position: &Position,
    text: &str,
    legal: &[Move],
) -> Result<Move, SanError> {
    let text = text.trim().trim_end_matches(['+', '#', '!', '?']);
    if text.is_empty() || !text.is_ascii() {
        return Err(SanError::Unreadable);
    }
    if let Some(long) = castling(text) {
        let king = position.king(position.side_to_move());
        let to = Square::new(if long { 2 } else { 6 }, king.rank());
        return only(
            legal
                .iter()
                .filter(|m| king.file() == 4 && m.from == king && Some(m.to) == to),
        );
    }
    let text: String = text.chars().filter(|&c| c != '-').collect();
    if let Some(uci) = Move::parse_uci(&text) {
        return only(legal.iter().filter(|m| {
            m.from == uci.from
                && m.to == uci.to
                && (uci.promotion.is_none() || m.promotion == uci.promotion)
        }));
    }
    let patterns = patterns(text.as_bytes());
    if patterns.is_empty() {
        return Err(SanError::Unreadable);
    }
    patterns
        .iter()
        .map(|p| only(legal.iter().filter(|m| p.matches(position, m))))
        .find(|found| *found != Err(SanError::Illegal))
        .unwrap_or(Err(SanError::Illegal))
}

/// The one move among `candidates`.
fn only<'a>(mut candidates: impl Iterator<Item = &'a Move>) -> Result<Move, SanError> {
    match (candidates.next(), candidates.next()) {
        (None, _) => Err(SanError::Illegal),
        (Some(&mv), None) => Ok(mv),
        (Some(_), Some(_)) => Err(SanError::Ambiguous),
    }
}

/// `Some(true)` for queenside castling however it's written, `Some(false)`
/// for kingside.
fn castling(text: &str) -> Option<bool> {
    let squeezed: String = text
        .chars()
        .filter(|&c| c != '-')
        .map(|c| match c {
            '0' => 'o',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    match squeezed.as_str() {
        "oo" => Some(false),
        "ooo" => Some(true),
        _ => None,
    }
}

/// What a SAN move says: which piece goes where, perhaps from which file
/// or rank, and what a pawn becomes.
#[derive(Debug)]
struct Pattern {
    piece: Piece,
    file: Option<u8>,
    rank: Option<u8>,
    to: Square,
    promotion: Option<Piece>,
}

impl Pattern {
    /// The pattern for `piece` with `from` written before the destination,
    /// if it makes sense: a file, a rank, both, or nothing.
    fn new(piece: Piece, from: &[u8], to: Square, promotion: Option<Piece>) -> Option<Pattern> {
        let (file, from) = match from {
            [f @ b'a'..=b'h', rest @ ..] => (Some(f - b'a'), rest),
            _ => (None, from),
        };
        let (rank, from) = match from {
            [r @ b'1'..=b'8', rest @ ..] => (Some(r - b'1'), rest),
            _ => (None, from),
        };
        // A pawn's own file is written only when it takes, onto another.
        let pawn_push_with_file = piece == Piece::Pawn && file == Some(to.file());
        let ok = from.is_empty()
            && (promotion.is_none() || piece == Piece::Pawn)
            && !pawn_push_with_file;
        ok.then_some(Pattern {
            piece,
            file,
            rank,
            to,
            promotion,
        })
    }

    fn matches(&self, position: &Position, mv: &Move) -> bool {
        mv.to == self.to
            && self.file.is_none_or(|f| mv.from.file() == f)
            && self.rank.is_none_or(|r| mv.from.rank() == r)
            && (self.promotion.is_none() || mv.promotion == self.promotion)
            && position
                .piece_at(mv.from)
                .is_some_and(|(_, p)| p == self.piece)
    }
}

/// What SAN text could mean, likeliest first: usually one pattern, two
/// when a lowercase `b` could be the b-pawn or a bishop, and none if it
/// isn't SAN.
fn patterns(text: &[u8]) -> Vec<Pattern> {
    let mut body = text;
    let mut promotion = None;
    if let [rest @ .., letter] = text
        && let Some(piece) = promotion_from_letter(*letter as char)
    {
        let rest = rest.strip_suffix(b"=").unwrap_or(rest);
        if rest.last().is_some_and(u8::is_ascii_digit) {
            promotion = Some(piece);
            body = rest;
        }
    }
    let [rest @ .., file, rank] = body else {
        return Vec::new();
    };
    let Some(to) = Square::from_chars(*file, *rank) else {
        return Vec::new();
    };
    let rest = rest
        .strip_suffix(b"x")
        .or_else(|| rest.strip_suffix(b":"))
        .unwrap_or(rest);

    let readings: Vec<(Piece, &[u8])> = match rest {
        [] => vec![(Piece::Pawn, rest)],
        [b'b', from @ ..] => vec![(Piece::Pawn, rest), (Piece::Bishop, from)],
        [b'a'..=b'h', ..] => vec![(Piece::Pawn, rest)],
        [letter, from @ ..] => match Piece::from_letter(*letter as char) {
            Some(piece) => vec![(piece, from)],
            None => Vec::new(),
        },
    };
    readings
        .into_iter()
        .filter_map(|(piece, from)| Pattern::new(piece, from, to, promotion))
        .collect()
}

#[cfg(test)]
mod tests {
    use rand::rngs::Xoshiro256PlusPlus;
    use rand::{RngExt, SeedableRng};

    use super::*;

    fn position(fen: &str) -> Position {
        Position::from_fen(fen).unwrap()
    }

    fn uci(text: &str) -> Move {
        Move::parse_uci(text).unwrap()
    }

    fn san(position: &Position, mv: &str) -> String {
        format(position, uci(mv)).unwrap()
    }

    #[test]
    fn formats_plain_moves() {
        let start = Position::start();
        assert_eq!(san(&start, "e2e4"), "e4");
        assert_eq!(san(&start, "g1f3"), "Nf3");
        assert_eq!(format(&start, uci("e2e5")), None);
    }

    #[test]
    fn formats_captures_en_passant_and_promotions() {
        let p = position("3qk3/2P5/8/3pP3/8/8/8/4K3 w - d6 0 2");
        assert_eq!(san(&p, "e5d6"), "exd6");
        assert_eq!(san(&p, "c7c8q"), "c8=Q", "the queen on d8 blocks");
        assert_eq!(san(&p, "c7d8n"), "cxd8=N");
        assert_eq!(san(&p, "c7d8q"), "cxd8=Q+");
        assert_eq!(san(&p, "e1e2"), "Ke2");
    }

    #[test]
    fn formats_castling_check_and_mate() {
        let p = position("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
        assert_eq!(san(&p, "e1g1"), "O-O");
        assert_eq!(san(&p, "e1c1"), "O-O-O");
        assert_eq!(san(&p, "a1a8"), "Rxa8+");

        let mut p = Position::start();
        for mv in ["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6"] {
            p.play(uci(mv)).unwrap();
        }
        assert_eq!(san(&p, "h5f7"), "Qxf7#");
        assert_eq!(san(&p, "c4f7"), "Bxf7+");
    }

    #[test]
    fn disambiguates_by_file_then_rank_then_both() {
        let knights = position("4k3/8/8/8/8/8/8/1N2KN2 w - - 0 1");
        assert_eq!(san(&knights, "b1d2"), "Nbd2");
        assert_eq!(san(&knights, "f1d2"), "Nfd2");
        assert_eq!(san(&knights, "b1c3"), "Nc3");

        let rooks = position("4k3/8/8/R7/8/8/8/R3K3 w - - 0 1");
        assert_eq!(san(&rooks, "a1a3"), "R1a3");
        assert_eq!(san(&rooks, "a5a3"), "R5a3");

        let queens = position("2k5/8/8/8/4Q2Q/8/8/K6Q w - - 0 1");
        assert_eq!(san(&queens, "h4e1"), "Qh4e1");
        assert_eq!(san(&queens, "e4e1"), "Qee1");
        assert_eq!(san(&queens, "h1e1"), "Q1e1");
    }

    #[test]
    fn reads_san_uci_and_castling() {
        let start = Position::start();
        for text in ["e4", "e2e4", "e2-e4", "Pe4", " e4 "] {
            assert_eq!(parse(&start, text), Ok(uci("e2e4")), "{text}");
        }
        for text in ["Nf3", "nf3", "Ng1f3", "Ng1-f3", "g1f3", "Nf3!?"] {
            assert_eq!(parse(&start, text), Ok(uci("g1f3")), "{text}");
        }

        let p = position("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
        for text in ["O-O", "0-0", "oo", "OO", "o-o+", "e1g1", "Kg1"] {
            assert_eq!(parse(&p, text), Ok(uci("e1g1")), "{text}");
        }
        for text in ["O-O-O", "0-0-0", "ooo", "e1c1"] {
            assert_eq!(parse(&p, text), Ok(uci("e1c1")), "{text}");
        }
        for text in ["Rxa8", "Ra8", "Ra1a8", "a1a8", "Rxa8+"] {
            assert_eq!(parse(&p, text), Ok(uci("a1a8")), "{text}");
        }
    }

    #[test]
    fn reads_promotions() {
        let p = position("3qk3/2P5/8/8/8/8/8/4K3 w - - 0 1");
        for text in ["c8=Q", "c8Q", "c8q", "c7c8q", "c7c8Q", "c8=Q+"] {
            assert_eq!(parse(&p, text), Ok(uci("c7c8q")), "{text}");
        }
        for text in ["cxd8=N", "cd8n", "c7d8n", "xd8=N"] {
            assert_eq!(parse(&p, text), Ok(uci("c7d8n")), "{text}");
        }
    }

    #[test]
    fn says_when_a_move_is_ambiguous() {
        let knights = position("4k3/8/8/8/8/8/8/1N2KN2 w - - 0 1");
        assert_eq!(parse(&knights, "Nd2"), Err(SanError::Ambiguous));
        assert_eq!(parse(&knights, "Nbd2"), Ok(uci("b1d2")));
        assert_eq!(parse(&knights, "N1d2"), Err(SanError::Ambiguous));

        let promotion = position("4k3/2P5/8/8/8/8/8/4K3 w - - 0 1");
        assert_eq!(parse(&promotion, "c8"), Err(SanError::Ambiguous));
        assert_eq!(parse(&promotion, "c7c8"), Err(SanError::Ambiguous));
    }

    #[test]
    fn a_lowercase_b_is_the_pawn_when_it_can_be() {
        let both = position("4k3/8/3B4/8/8/n7/1P6/4K3 w - - 0 1");
        assert_eq!(parse(&both, "bxa3"), Ok(uci("b2a3")));
        assert_eq!(parse(&both, "Bxa3"), Ok(uci("d6a3")));
        assert_eq!(parse(&both, "bb4"), Ok(uci("d6b4")));
        assert_eq!(parse(&both, "b3"), Ok(uci("b2b3")));

        let bishop_only = position("4k3/8/3B4/8/8/n7/8/4K3 w - - 0 1");
        assert_eq!(parse(&bishop_only, "bxa3"), Ok(uci("d6a3")));
        assert_eq!(parse(&bishop_only, "bc7"), Ok(uci("d6c7")));
    }

    #[test]
    fn says_when_a_move_is_illegal_or_unreadable() {
        let start = Position::start();
        for text in ["e5", "Nf6", "O-O", "Ke2", "e2e5", "e4=Q", "e2e4q", "Ree4"] {
            assert_eq!(parse(&start, text), Err(SanError::Illegal), "{text}");
        }
        for text in ["", "+", "hello", "Zf3", "e9", "E4", "Nf3x", "Qxé4", "1e4"] {
            assert_eq!(parse(&start, text), Err(SanError::Unreadable), "{text}");
        }
        // Castling needs the king on its e-file square.
        let moved = position("4k3/8/8/8/8/8/8/5K1R w - - 0 1");
        assert_eq!(parse(&moved, "O-O"), Err(SanError::Illegal));
    }

    /// cozy-chess's own SAN writer, for comparison.
    fn cozy_san(position: &Position, mv: Move) -> String {
        let board = position.cozy();
        let mv = cozy_chess::util::parse_uci_move(board, &mv.to_string()).unwrap();
        cozy_chess::util::display_san_move(board, mv).to_string()
    }

    #[test]
    fn every_move_of_random_games_round_trips() {
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(0xC0FFEE);
        let mut moves_checked = 0;
        for _ in 0..250 {
            let mut position = Position::start();
            for _ in 0..120 {
                let legal = position.legal_moves();
                if legal.is_empty() || position.halfmove_clock() >= 100 {
                    break;
                }
                for &mv in &legal {
                    let text = format_among(&position, mv, &legal).unwrap();
                    let back = parse_among(&position, &text, &legal);
                    assert_eq!(back, Ok(mv), "{text} in {}", position.fen());
                    assert_eq!(text, cozy_san(&position, mv), "in {}", position.fen());
                    moves_checked += 1;
                }
                let mv = legal[rng.random_range(0..legal.len())];
                assert_eq!(parse_among(&position, &mv.to_string(), &legal), Ok(mv));
                position.play(mv).unwrap();
            }
        }
        assert!(moves_checked > 800_000, "{moves_checked}");
    }
}
