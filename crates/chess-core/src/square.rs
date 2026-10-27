//! The 64 squares, a1 to h8.

use std::fmt;

/// A square on the board, numbered 0 (a1) to 63 (h8) along each rank in
/// turn: a1, b1, … h1, a2, ….
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Square(u8);

impl Square {
    /// The square on `file` (0 is the a-file) and `rank` (0 is the first
    /// rank). `None` off the board.
    pub fn new(file: u8, rank: u8) -> Option<Square> {
        (file < 8 && rank < 8).then(|| Square(rank * 8 + file))
    }

    /// The square numbered `index`, or `None` past 63.
    pub fn from_index(index: u8) -> Option<Square> {
        (index < 64).then_some(Square(index))
    }

    pub fn index(self) -> u8 {
        self.0
    }

    /// 0 is the a-file, 7 the h-file.
    pub fn file(self) -> u8 {
        self.0 % 8
    }

    /// 0 is the first rank, 7 the eighth.
    pub fn rank(self) -> u8 {
        self.0 / 8
    }

    /// Light squares, like h1, as opposed to dark ones, like a1.
    pub fn is_light(self) -> bool {
        (self.file() + self.rank()) % 2 == 1
    }

    /// The square a name like `e4` names.
    pub fn parse(name: &str) -> Option<Square> {
        match name.as_bytes() {
            &[file, rank] => Square::from_chars(file, rank),
            _ => None,
        }
    }

    /// The square a file letter and a rank digit name.
    pub(crate) fn from_chars(file: u8, rank: u8) -> Option<Square> {
        Square::new(file.wrapping_sub(b'a'), rank.wrapping_sub(b'1'))
    }

    pub(crate) fn file_char(self) -> char {
        (b'a' + self.file()) as char
    }

    pub(crate) fn rank_char(self) -> char {
        (b'1' + self.rank()) as char
    }

    pub(crate) fn to_cozy(self) -> cozy_chess::Square {
        cozy_chess::Square::index(self.0 as usize)
    }

    pub(crate) fn from_cozy(square: cozy_chess::Square) -> Self {
        Square(square as u8)
    }
}

impl fmt::Display for Square {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.file_char(), self.rank_char())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for index in 0..64 {
            let square = Square::from_index(index).unwrap();
            assert_eq!(Square::parse(&square.to_string()), Some(square));
            assert_eq!(Square::from_cozy(square.to_cozy()), square);
            assert_eq!(square.to_cozy().to_string(), square.to_string());
        }
        assert_eq!(Square::parse("a1").unwrap().index(), 0);
        assert_eq!(Square::parse("h8").unwrap().index(), 63);
        assert_eq!(Square::parse("e4"), Square::new(4, 3));
    }

    #[test]
    fn rejects_what_is_off_the_board() {
        for name in ["", "e", "e9", "i1", "e44", "E4", "4e"] {
            assert_eq!(Square::parse(name), None, "{name}");
        }
        assert_eq!(Square::new(8, 0), None);
        assert_eq!(Square::from_index(64), None);
    }

    #[test]
    fn a1_is_dark_and_h1_light() {
        assert!(!Square::parse("a1").unwrap().is_light());
        assert!(Square::parse("h1").unwrap().is_light());
        assert!(Square::parse("d1").unwrap().is_light());
        assert!(!Square::parse("e1").unwrap().is_light());
    }
}
