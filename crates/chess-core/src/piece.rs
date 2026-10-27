//! The two sides and the six kinds of piece.

use std::ops::Not;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Color {
    White,
    Black,
}

impl Color {
    pub const BOTH: [Color; 2] = [Color::White, Color::Black];

    /// 0 for white, 1 for black: where this side sits in a pair.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Color::White => "white",
            Color::Black => "black",
        }
    }

    pub(crate) fn to_cozy(self) -> cozy_chess::Color {
        match self {
            Color::White => cozy_chess::Color::White,
            Color::Black => cozy_chess::Color::Black,
        }
    }

    pub(crate) fn from_cozy(color: cozy_chess::Color) -> Self {
        match color {
            cozy_chess::Color::White => Color::White,
            cozy_chess::Color::Black => Color::Black,
        }
    }
}

impl Not for Color {
    type Output = Color;

    fn not(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Piece {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

impl Piece {
    /// What a pawn can become, best first.
    pub const PROMOTIONS: [Piece; 4] = [Piece::Queen, Piece::Rook, Piece::Bishop, Piece::Knight];

    /// The capital letter SAN writes for this piece; a pawn's is `P`, which
    /// SAN leaves out.
    pub fn letter(self) -> char {
        match self {
            Piece::Pawn => 'P',
            Piece::Knight => 'N',
            Piece::Bishop => 'B',
            Piece::Rook => 'R',
            Piece::Queen => 'Q',
            Piece::King => 'K',
        }
    }

    /// The piece a letter names, in either case.
    pub fn from_letter(letter: char) -> Option<Piece> {
        Some(match letter.to_ascii_uppercase() {
            'P' => Piece::Pawn,
            'N' => Piece::Knight,
            'B' => Piece::Bishop,
            'R' => Piece::Rook,
            'Q' => Piece::Queen,
            'K' => Piece::King,
            _ => return None,
        })
    }

    pub(crate) fn to_cozy(self) -> cozy_chess::Piece {
        match self {
            Piece::Pawn => cozy_chess::Piece::Pawn,
            Piece::Knight => cozy_chess::Piece::Knight,
            Piece::Bishop => cozy_chess::Piece::Bishop,
            Piece::Rook => cozy_chess::Piece::Rook,
            Piece::Queen => cozy_chess::Piece::Queen,
            Piece::King => cozy_chess::Piece::King,
        }
    }

    pub(crate) fn from_cozy(piece: cozy_chess::Piece) -> Self {
        match piece {
            cozy_chess::Piece::Pawn => Piece::Pawn,
            cozy_chess::Piece::Knight => Piece::Knight,
            cozy_chess::Piece::Bishop => Piece::Bishop,
            cozy_chess::Piece::Rook => Piece::Rook,
            cozy_chess::Piece::Queen => Piece::Queen,
            cozy_chess::Piece::King => Piece::King,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_round_trip_in_either_case() {
        for piece in [
            Piece::Pawn,
            Piece::Knight,
            Piece::Bishop,
            Piece::Rook,
            Piece::Queen,
            Piece::King,
        ] {
            assert_eq!(Piece::from_letter(piece.letter()), Some(piece));
            assert_eq!(
                Piece::from_letter(piece.letter().to_ascii_lowercase()),
                Some(piece)
            );
            assert_eq!(Piece::from_cozy(piece.to_cozy()), piece);
        }
        assert_eq!(Piece::from_letter('x'), None);
    }

    #[test]
    fn sides_swap_and_index() {
        assert_eq!(!Color::White, Color::Black);
        assert_eq!(!Color::Black, Color::White);
        assert_eq!(Color::White.index(), 0);
        assert_eq!(Color::Black.index(), 1);
        for color in Color::BOTH {
            assert_eq!(Color::from_cozy(color.to_cozy()), color);
        }
    }
}
