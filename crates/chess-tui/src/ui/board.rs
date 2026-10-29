//! The board: squares in the player's colors, the pieces, and marks for
//! the last move, a check, the cursor and where a piece can go.

use chess_core::moves::Move;
use chess_core::piece::{self, Piece};
use chess_core::position::Position;
use chess_core::square::Square;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use shell_tui::Pictures;

use super::pictures;
use crate::app::{Hit, Hits};
use crate::prefs::{BoardLook, PieceStyle, Prefs};

/// A square's size on screen, in columns and rows.
pub type SquareSize = (u16, u16);

/// Square sizes from biggest: twice 80×24's for terminals of about
/// 120×34, down to the smallest.
pub const SQUARES: [SquareSize; 5] = [(10, 4), (7, 3), (5, 2), (3, 1), (2, 1)];

/// The smallest square a piece is drawn as a picture on.
const PICTURED: SquareSize = (5, 2);

/// What else drawing the board does, besides its cells.
#[derive(Clone, Copy, Debug, Default)]
pub struct Extras<'a> {
    /// Pieces go here as pictures, too.
    pub pictures: Option<&'a Pictures>,
    /// The squares go here, to be clicked.
    pub hits: Option<&'a Hits>,
}

/// The board's size on screen, with its files and ranks when `coords`.
pub fn board_size(square: SquareSize, coords: bool) -> (u16, u16) {
    let (w, h) = square;
    (
        w * 8 + if coords { 2 } else { 0 },
        h * 8 + u16::from(coords),
    )
}

/// What stands out on the board.
#[derive(Clone, Debug, Default)]
pub struct Marks {
    pub last: Option<Move>,
    /// The king in check.
    pub check: Option<Square>,
    pub cursor: Option<Square>,
    /// The piece picked up, or the one a typed move must be.
    pub from: Vec<Square>,
    /// Where it can go.
    pub to: Vec<Square>,
}

/// Light and dark squares.
fn squares(look: BoardLook) -> (Color, Color) {
    match look {
        BoardLook::Wood => (Color::Indexed(223), Color::Indexed(137)),
        BoardLook::Green => (Color::Indexed(187), Color::Indexed(65)),
        BoardLook::Blue => (Color::Indexed(152), Color::Indexed(67)),
        BoardLook::Gray => (Color::Indexed(250), Color::Indexed(244)),
    }
}

/// The last move's squares, light and dark.
const LAST: (Color, Color) = (Color::Indexed(186), Color::Indexed(143));
const FROM: Color = Color::Indexed(113);
const TARGET: Color = Color::Indexed(174);
const CURSOR: Color = Color::Indexed(117);
const CHECK: Color = Color::Indexed(167);
const DOT: Color = Color::Indexed(58);

pub fn piece_color(color: piece::Color) -> Color {
    match color {
        piece::Color::White => Color::Indexed(231),
        piece::Color::Black => Color::Indexed(16),
    }
}

/// A piece as `style` draws it.
pub fn symbol(style: PieceStyle, color: piece::Color, piece: Piece) -> &'static str {
    let at = |set: [&'static str; 6]| match piece {
        Piece::King => set[0],
        Piece::Queen => set[1],
        Piece::Rook => set[2],
        Piece::Bishop => set[3],
        Piece::Knight => set[4],
        Piece::Pawn => set[5],
    };
    match (style, color) {
        // The solid glyphs for both sides: color tells them apart.
        (PieceStyle::Glyphs, _) => at(["♚", "♛", "♜", "♝", "♞", "♟"]),
        (PieceStyle::Letters, piece::Color::White) => at(["K", "Q", "R", "B", "N", "P"]),
        (PieceStyle::Letters, piece::Color::Black) => at(["k", "q", "r", "b", "n", "p"]),
    }
}

/// The square drawn at column `col` and row `row` from the top left, with
/// `bottom`'s pieces at the bottom.
pub fn square_at(col: u16, row: u16, bottom: piece::Color) -> Option<Square> {
    let (col, row) = (col as u8, row as u8);
    match bottom {
        piece::Color::White => Square::new(col, 7 - row),
        piece::Color::Black => Square::new(7 - col, row),
    }
}

fn put(buf: &mut Buffer, x: u16, y: u16, symbol: &str, style: Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.reset();
        cell.set_symbol(symbol).set_style(style);
    }
}

/// Draws `position` with its top left at `at`, its squares `square` big,
/// as `prefs` say, and `bottom`'s side at the bottom.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    buf: &mut Buffer,
    at: (u16, u16),
    position: &Position,
    square: SquareSize,
    bottom: piece::Color,
    prefs: &Prefs,
    marks: &Marks,
    extras: Extras,
) {
    let (w, h) = square;
    let (light, dark) = squares(prefs.board);
    let left = at.0 + if prefs.coordinates { 2 } else { 0 };
    let label = Style::default().fg(Color::DarkGray);
    let pictures = extras
        .pictures
        .filter(|_| prefs.images && w >= PICTURED.0 && h >= PICTURED.1);
    for row in 0..8 {
        for col in 0..8 {
            let Some(sq) = square_at(col, row, bottom) else {
                continue;
            };
            let is_light = sq.is_light();
            let last = marks.last.is_some_and(|m| m.from == sq || m.to == sq);
            let piece = position.piece_at(sq);
            let bg = if marks.cursor == Some(sq) {
                CURSOR
            } else if marks.check == Some(sq) {
                CHECK
            } else if marks.from.contains(&sq) {
                FROM
            } else if marks.to.contains(&sq) && piece.is_some() {
                TARGET
            } else if last {
                if is_light { LAST.0 } else { LAST.1 }
            } else if is_light {
                light
            } else {
                dark
            };
            let (x, y) = (left + col * w, at.1 + row * h);
            for dy in 0..h {
                for dx in 0..w {
                    put(buf, x + dx, y + dy, " ", Style::default().bg(bg));
                }
            }
            let mid = x + (w - 1) / 2;
            // Pieces sit on the square's top row, or its middle when it's
            // tall.
            let top = y + (h - 1) / 2;
            match piece {
                Some((color, kind)) => {
                    let style = Style::default()
                        .fg(piece_color(color))
                        .bg(bg)
                        .add_modifier(Modifier::BOLD);
                    put(buf, mid, top, symbol(prefs.pieces, color, kind), style);
                }
                None if marks.to.contains(&sq) => {
                    put(buf, mid, top, "•", Style::default().fg(DOT).bg(bg));
                }
                None => {}
            }
            let area = Rect::new(x, y, w, h);
            if let (Some(pictures), Some((color, kind))) = (pictures, piece) {
                let size = pictures::pixels(square, pictures.cell);
                pictures.place(buf, area, pictures::picture(color, kind, bg, size));
            }
            if let Some(hits) = extras.hits {
                hits.add(area, Hit::Square(sq));
            }
        }
        if prefs.coordinates
            && let Some(sq) = square_at(0, row, bottom)
        {
            let rank = (b'1' + sq.rank()) as char;
            put(
                buf,
                at.0,
                at.1 + row * h + (h - 1) / 2,
                &rank.to_string(),
                label,
            );
        }
    }
    if prefs.coordinates {
        for col in 0..8 {
            if let Some(sq) = square_at(col, 0, bottom) {
                let file = (b'a' + sq.file()) as char;
                put(
                    buf,
                    left + col * w + (w - 1) / 2,
                    at.1 + 8 * h,
                    &file.to_string(),
                    label,
                );
            }
        }
    }
}

/// The board's area on screen, for drawing over it.
pub fn area_of(at: (u16, u16), square: SquareSize, coords: bool) -> Rect {
    let (width, height) = board_size(square, coords);
    Rect {
        x: at.0,
        y: at.1,
        width,
        height,
    }
}
