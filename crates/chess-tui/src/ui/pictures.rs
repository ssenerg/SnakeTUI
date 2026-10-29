//! Pieces as pictures, for terminals that show them: Cburnett's set (see
//! `assets/pieces/LICENSE`), drawn over the square's color at the square's
//! size in pixels.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chess_core::piece::{self, Piece};
use ratatui::style::Color;
use resvg::tiny_skia::{self, Pixmap, Transform};
use resvg::usvg::{Options, Tree};
use shell_tui::Picture;

/// The drawings, white's then black's, king to pawn.
const SVGS: [[&str; 6]; 2] = [
    [
        include_str!("../../assets/pieces/Chess_klt45.svg"),
        include_str!("../../assets/pieces/Chess_qlt45.svg"),
        include_str!("../../assets/pieces/Chess_rlt45.svg"),
        include_str!("../../assets/pieces/Chess_blt45.svg"),
        include_str!("../../assets/pieces/Chess_nlt45.svg"),
        include_str!("../../assets/pieces/Chess_plt45.svg"),
    ],
    [
        include_str!("../../assets/pieces/Chess_kdt45.svg"),
        include_str!("../../assets/pieces/Chess_qdt45.svg"),
        include_str!("../../assets/pieces/Chess_rdt45.svg"),
        include_str!("../../assets/pieces/Chess_bdt45.svg"),
        include_str!("../../assets/pieces/Chess_ndt45.svg"),
        include_str!("../../assets/pieces/Chess_pdt45.svg"),
    ],
];

/// The drawings' own size: each is 45 units square.
const DRAWN: f32 = 45.0;
/// The widest picture made, in pixels; bigger squares are scaled up by the
/// terminal.
const MAX_WIDTH: u32 = 160;
/// Pictures kept made: enough for every piece on every square color of
/// a few boards and sizes.
const KEPT: usize = 1024;

fn index(piece: Piece) -> usize {
    match piece {
        Piece::King => 0,
        Piece::Queen => 1,
        Piece::Rook => 2,
        Piece::Bishop => 3,
        Piece::Knight => 4,
        Piece::Pawn => 5,
    }
}

/// The drawing of `color`'s `piece`, read once.
fn tree(color: piece::Color, piece: Piece) -> &'static Tree {
    static TREES: OnceLock<Vec<Tree>> = OnceLock::new();
    let trees = TREES.get_or_init(|| {
        SVGS.iter()
            .flatten()
            .map(|svg| Tree::from_str(svg, &Options::default()).expect("the pieces' SVGs parse"))
            .collect()
    });
    let side = usize::from(color == piece::Color::Black);
    &trees[side * 6 + index(piece)]
}

/// `color` as red, green and blue, by the xterm palette for indexed
/// colors.
pub fn rgb(color: Color) -> [u8; 3] {
    const BASIC: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 0, 0],
        [0, 205, 0],
        [205, 205, 0],
        [0, 0, 238],
        [205, 0, 205],
        [0, 205, 205],
        [229, 229, 229],
        [127, 127, 127],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [92, 92, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let indexed = |i: u8| match i {
        0..=15 => BASIC[usize::from(i)],
        16..=231 => {
            let i = i - 16;
            [
                LEVELS[usize::from(i / 36)],
                LEVELS[usize::from(i / 6 % 6)],
                LEVELS[usize::from(i % 6)],
            ]
        }
        _ => [8 + 10 * (i - 232); 3],
    };
    match color {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(i) => indexed(i),
        Color::Black | Color::Reset => BASIC[0],
        Color::Red => BASIC[1],
        Color::Green => BASIC[2],
        Color::Yellow => BASIC[3],
        Color::Blue => BASIC[4],
        Color::Magenta => BASIC[5],
        Color::Cyan => BASIC[6],
        Color::Gray => BASIC[7],
        Color::DarkGray => BASIC[8],
        Color::LightRed => BASIC[9],
        Color::LightGreen => BASIC[10],
        Color::LightYellow => BASIC[11],
        Color::LightBlue => BASIC[12],
        Color::LightMagenta => BASIC[13],
        Color::LightCyan => BASIC[14],
        Color::White => BASIC[15],
    }
}

/// A square's size in pixels: `cells` of `cell` pixels each, no wider than
/// [`MAX_WIDTH`].
pub fn pixels(cells: (u16, u16), cell: (u16, u16)) -> (u32, u32) {
    let w = u32::from(cells.0) * u32::from(cell.0.max(1));
    let h = u32::from(cells.1) * u32::from(cell.1.max(1));
    if w <= MAX_WIDTH {
        (w.max(1), h.max(1))
    } else {
        (MAX_WIDTH, (h * MAX_WIDTH / w).max(1))
    }
}

/// `color`'s `piece` on a square of `bg`, `size` pixels big: made once,
/// then kept.
pub fn picture(color: piece::Color, piece: Piece, bg: Color, size: (u32, u32)) -> Picture {
    static KEPT_PICTURES: OnceLock<Mutex<HashMap<u64, Picture>>> = OnceLock::new();
    let [r, g, b] = rgb(bg);
    let (w, h) = (size.0.clamp(1, 4095), size.1.clamp(1, 4095));
    // Every part of what's drawn, packed: no two pictures share an id.
    let id = u64::from(color == piece::Color::Black)
        | (index(piece) as u64) << 1
        | u64::from(u32::from_be_bytes([0, r, g, b])) << 4
        | u64::from(w) << 28
        | u64::from(h) << 40;
    let kept = KEPT_PICTURES.get_or_init(Mutex::default);
    if let Some(picture) = kept.lock().unwrap().get(&id) {
        return picture.clone();
    }
    let picture = Picture {
        id,
        png: draw(tree(color, piece), [r, g, b], (w, h)).into(),
    };
    let mut kept = kept.lock().unwrap();
    if kept.len() >= KEPT {
        kept.clear();
    }
    kept.insert(id, picture.clone());
    picture
}

/// `tree` centered on a `bg` square `size` pixels big, as PNG.
fn draw(tree: &Tree, [r, g, b]: [u8; 3], (w, h): (u32, u32)) -> Vec<u8> {
    let mut pixmap = Pixmap::new(w, h).expect("a square has pixels");
    pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
    let scale = w.min(h) as f32 / DRAWN;
    let (dx, dy) = (
        (w as f32 - DRAWN * scale) / 2.0,
        (h as f32 - DRAWN * scale) / 2.0,
    );
    let at = Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(tree, at, &mut pixmap.as_mut());
    pixmap.encode_png().expect("a pixmap encodes as PNG")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_piece_is_drawn_on_its_square_at_any_size() {
        for color in [piece::Color::White, piece::Color::Black] {
            for piece in [
                Piece::King,
                Piece::Queen,
                Piece::Rook,
                Piece::Bishop,
                Piece::Knight,
                Piece::Pawn,
            ] {
                for size in [(10, 8), (80, 64), (160, 128)] {
                    let p = picture(color, piece, Color::Indexed(137), size);
                    assert_eq!(&p.png[1..4], b"PNG");
                    let pixmap = Pixmap::decode_png(&p.png).unwrap();
                    assert_eq!((pixmap.width(), pixmap.height()), size);
                    // The corners are the square's.
                    let corner = pixmap.pixel(0, 0).unwrap();
                    assert_eq!(
                        (corner.red(), corner.green(), corner.blue()),
                        (175, 135, 95)
                    );
                }
            }
        }
        let white = picture(
            piece::Color::White,
            Piece::Knight,
            Color::Indexed(223),
            (80, 64),
        );
        let black = picture(
            piece::Color::Black,
            Piece::Knight,
            Color::Indexed(223),
            (80, 64),
        );
        let again = picture(
            piece::Color::White,
            Piece::Knight,
            Color::Indexed(223),
            (80, 64),
        );
        assert_ne!(white.id, black.id);
        assert_eq!(white, again, "kept, not drawn twice");
    }

    #[test]
    fn colors_and_sizes_are_worked_out() {
        assert_eq!(rgb(Color::Indexed(16)), [0, 0, 0]);
        assert_eq!(rgb(Color::Indexed(223)), [255, 215, 175]);
        assert_eq!(rgb(Color::Indexed(244)), [128, 128, 128]);
        assert_eq!(rgb(Color::Rgb(1, 2, 3)), [1, 2, 3]);
        assert_eq!(pixels((5, 2), (8, 16)), (40, 32));
        assert_eq!(pixels((10, 4), (20, 40)), (160, 128), "no wider than 160");
    }
}
