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
const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// How one snake is drawn in multiplayer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnakeLook {
    pub head: Color,
    pub body: Color,
    /// Drawn on the head, so color isn't the only way to tell snakes apart.
    pub number: Option<u8>,
}

/// Draws a board straight into the buffer, two terminal columns per cell.
/// Heads go on top of bodies, bodies on top of food, food on top of walls.
pub struct BoardWidget<'a> {
    pub view: &'a MatchView,
    pub theme: &'a Theme,
    /// The board cell drawn at the area's top-left corner, for boards bigger
    /// than the area.
    pub origin: Point,
    /// Per snake id. Without it, every snake gets the theme's colors.
    pub looks: Option<&'a [SnakeLook]>,
}

impl<'a> BoardWidget<'a> {
    pub fn new(view: &'a MatchView, theme: &'a Theme) -> Self {
        Self {
            view,
            theme,
            origin: Point { x: 0, y: 0 },
            looks: None,
        }
    }
}

impl Widget for BoardWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let Self {
            view,
            theme,
            origin,
            looks,
        } = self;
        let bg = theme_color(theme, ColorSlot::Background);
        let mut paint = |p: Point, text: [&str; 2], style: Style| {
            if p.x < origin.x || p.y < origin.y {
                return;
            }
            let y = u32::from(area.y) + u32::from(p.y - origin.y);
            for (i, symbol) in text.into_iter().enumerate() {
                let x = u32::from(area.x) + u32::from(p.x - origin.x) * 2 + i as u32;
                if x >= u32::from(area.right()) || y >= u32::from(area.bottom()) {
                    continue;
                }
                if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
                    cell.reset();
                    cell.set_symbol(symbol).set_style(style);
                }
            }
        };
        let plain = |fg: Color| Style::default().fg(fg).bg(bg);

        let wall = theme_color(theme, ColorSlot::Wall);
        for y in 0..view.height {
            for x in 0..view.width {
                let p = Point { x, y };
                if view.wall_at(x, y) {
                    paint(p, BLOCK, plain(wall));
                } else {
                    paint(p, EMPTY, plain(bg));
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
            let style = plain(theme_color(theme, color)).add_modifier(Modifier::BOLD);
            paint(p, [theme.icon(icon), " "], style);
        }
        let look = |id: u8| {
            looks
                .and_then(|l| l.get(id as usize).copied())
                .unwrap_or(SnakeLook {
                    head: theme_color(theme, ColorSlot::Head),
                    body: theme_color(theme, ColorSlot::Body),
                    number: None,
                })
        };
        for snake in &view.snakes {
            let body = look(snake.id).body;
            for &p in snake.body.iter().skip(1) {
                paint(p, BLOCK, plain(body));
            }
        }
        for snake in &view.snakes {
            let Some(&p) = snake.body.first() else {
                continue;
            };
            let look = look(snake.id);
            match look.number {
                Some(n) => {
                    let style = Style::default()
                        .fg(Color::Black)
                        .bg(look.head)
                        .add_modifier(Modifier::BOLD);
                    paint(p, [DIGITS[n as usize % 10], " "], style);
                }
                None => paint(p, BLOCK, plain(look.head)),
            }
        }
    }
}
