//! A move as a player means it: castling is the king's two-square step, as
//! in `e1g1`, never cozy-chess's king-takes-own-rook.

use std::fmt;

use crate::piece::Piece;
use crate::square::Square;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Move {
    pub from: Square,
    pub to: Square,
    /// What a pawn reaching the last rank becomes.
    pub promotion: Option<Piece>,
}

impl Move {
    pub fn new(from: Square, to: Square) -> Move {
        Move {
            from,
            to,
            promotion: None,
        }
    }

    /// The move UCI text like `e2e4` or `e7e8q` names, whether or not it's
    /// legal anywhere. The promotion letter may be either case.
    pub fn parse_uci(text: &str) -> Option<Move> {
        let bytes = text.as_bytes();
        if !(4..=5).contains(&bytes.len()) {
            return None;
        }
        let promotion = match bytes.get(4) {
            None => None,
            Some(&letter) => Some(promotion_from_letter(letter as char)?),
        };
        Some(Move {
            from: Square::from_chars(bytes[0], bytes[1])?,
            to: Square::from_chars(bytes[2], bytes[3])?,
            promotion,
        })
    }

    /// The move in 16 bits: `from` in the low six, `to` in the next six,
    /// and the promotion in three above them (0 for none, then knight,
    /// bishop, rook, queen). A game's moves are kept this way.
    pub fn pack(self) -> u16 {
        let promotion = match self.promotion {
            None => 0,
            Some(Piece::Knight) => 1,
            Some(Piece::Bishop) => 2,
            Some(Piece::Rook) => 3,
            Some(Piece::Queen) => 4,
            // Pawns and kings aren't promotions; [`Position`] never makes them.
            Some(Piece::Pawn | Piece::King) => 7,
        };
        self.from.index() as u16 | ((self.to.index() as u16) << 6) | (promotion << 12)
    }

    /// The move [`Move::pack`] made `bits` from; `None` for bits it never
    /// makes.
    pub fn unpack(bits: u16) -> Option<Move> {
        let promotion = match bits >> 12 {
            0 => None,
            1 => Some(Piece::Knight),
            2 => Some(Piece::Bishop),
            3 => Some(Piece::Rook),
            4 => Some(Piece::Queen),
            _ => return None,
        };
        Some(Move {
            from: Square::from_index((bits & 63) as u8)?,
            to: Square::from_index(((bits >> 6) & 63) as u8)?,
            promotion,
        })
    }
}

/// A promotion piece from its letter, in either case.
pub(crate) fn promotion_from_letter(letter: char) -> Option<Piece> {
    Piece::from_letter(letter).filter(|piece| Piece::PROMOTIONS.contains(piece))
}

/// UCI text: `e2e4`, `e7e8q`, `e1g1`.
impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.from, self.to)?;
        if let Some(piece) = self.promotion {
            write!(f, "{}", piece.letter().to_ascii_lowercase())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uci(text: &str) -> Move {
        Move::parse_uci(text).unwrap()
    }

    #[test]
    fn uci_round_trips() {
        for text in ["e2e4", "e7e8q", "a2a1n", "e1g1", "h8a1"] {
            assert_eq!(uci(text).to_string(), text);
        }
        assert_eq!(uci("e7e8Q"), uci("e7e8q"));
    }

    #[test]
    fn uci_rejects_nonsense() {
        for text in ["", "e2", "e2e", "e2e9", "e7e8k", "e7e8p", "e2e4qq", "e2-e4"] {
            assert_eq!(Move::parse_uci(text), None, "{text}");
        }
    }

    #[test]
    fn every_move_packs_and_unpacks() {
        let promotions = [None]
            .into_iter()
            .chain(Piece::PROMOTIONS.into_iter().map(Some));
        for promotion in promotions {
            for from in 0..64 {
                for to in 0..64 {
                    let mv = Move {
                        from: Square::from_index(from).unwrap(),
                        to: Square::from_index(to).unwrap(),
                        promotion,
                    };
                    assert_eq!(Move::unpack(mv.pack()), Some(mv));
                }
            }
        }
    }

    #[test]
    fn unpack_rejects_bits_pack_never_makes() {
        assert_eq!(Move::unpack(5 << 12), None);
        assert_eq!(Move::unpack(1 << 15), None);
    }
}
