use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use snake_core::config::{ColorSlot, IconSlot, Theme};
use snake_core::geom::Point;
use snake_core::view::MatchView;
use snake_core::world::FoodKind;

use crate::palette::theme_color;

const BLOCK: [&str; 2] = ["█", "█"];
const EMPTY: [&str; 2] = [" ", " "];

/// Draws a board straight into the buffer, two terminal columns per cell.
/// Heads go on top of bodies, bodies on top of food, food on top of walls.
pub struct BoardWidget<'a> {
    pub view: &'a MatchView,
    pub theme: &'a Theme,
}

impl Widget for BoardWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let Self { view, theme } = self;
        let bg = theme_color(theme, ColorSlot::Background);
        let mut paint = |p: Point, text: [&str; 2], fg: Color, bold: bool| {
            let mut style = Style::default().fg(fg).bg(bg);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            let y = area.y + p.y;
            for (i, symbol) in text.into_iter().enumerate() {
                let x = area.x + p.x * 2 + i as u16;
                if x >= area.right() || y >= area.bottom() {
                    continue;
                }
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_symbol(symbol).set_style(style);
                }
            }
        };

        let wall = theme_color(theme, ColorSlot::Wall);
        for y in 0..view.height {
            for x in 0..view.width {
                let p = Point { x, y };
                if view.wall_at(x, y) {
                    paint(p, BLOCK, wall, false);
                } else {
                    paint(p, EMPTY, bg, false);
                }
            }
        }
        for &(p, kind) in &view.foods {
            let (icon, color) = match kind {
                FoodKind::Normal => (IconSlot::Food, ColorSlot::Food),
                FoodKind::Super => (IconSlot::SuperFood, ColorSlot::SuperFood),
                FoodKind::Poison => (IconSlot::Poison, ColorSlot::Poison),
            };
            // The glyph, padded to the cell's two columns.
            paint(p, [theme.icon(icon), " "], theme_color(theme, color), true);
        }
        let body = theme_color(theme, ColorSlot::Body);
        for snake in &view.snakes {
            for &p in snake.body.iter().skip(1) {
                paint(p, BLOCK, body, false);
            }
        }
        let head = theme_color(theme, ColorSlot::Head);
        for snake in &view.snakes {
            if let Some(&p) = snake.body.first() {
                paint(p, BLOCK, head, false);
            }
        }
    }
}
