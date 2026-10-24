//! `BoardWidget` replaced a paragraph of styled spans; it must draw exactly
//! what that did, cell for cell.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use snake_core::config::{ColorSlot, IconSlot, InnerWalls, Rules, Theme, WallMode};
use snake_core::view::MatchView;
use snake_core::world::{FoodKind, World};
use snake_tui::palette::theme_color;
use snake_tui::ui::BoardWidget;

/// The board drawing from v0.1.2, reading a `MatchView`.
fn old_board_lines(view: &MatchView, theme: &Theme) -> Vec<Line<'static>> {
    #[derive(Clone, Copy)]
    enum Cell {
        Empty,
        Wall,
        Food(FoodKind),
        Body,
        Head,
    }
    let (w, h) = (view.width as usize, view.height as usize);
    let mut grid = vec![Cell::Empty; w * h];
    for y in 0..view.height {
        for x in 0..view.width {
            if view.wall_at(x, y) {
                grid[y as usize * w + x as usize] = Cell::Wall;
            }
        }
    }
    for &(p, kind) in &view.foods {
        grid[p.y as usize * w + p.x as usize] = Cell::Food(kind);
    }
    for snake in &view.snakes {
        for p in snake.body.iter().skip(1) {
            grid[p.y as usize * w + p.x as usize] = Cell::Body;
        }
    }
    for snake in &view.snakes {
        if let Some(head) = snake.body.first() {
            grid[head.y as usize * w + head.x as usize] = Cell::Head;
        }
    }
    let bg = theme_color(theme, ColorSlot::Background);
    (0..h)
        .map(|y| {
            let spans: Vec<Span> = (0..w)
                .map(|x| {
                    let (text, fg, bold) = match grid[y * w + x] {
                        Cell::Empty => ("  ".to_string(), bg, false),
                        Cell::Wall => ("██".into(), theme_color(theme, ColorSlot::Wall), false),
                        Cell::Head => ("██".into(), theme_color(theme, ColorSlot::Head), false),
                        Cell::Body => ("██".into(), theme_color(theme, ColorSlot::Body), false),
                        Cell::Food(kind) => {
                            let (icon, color) = match kind {
                                FoodKind::Normal => (IconSlot::Food, ColorSlot::Food),
                                FoodKind::Super => (IconSlot::SuperFood, ColorSlot::SuperFood),
                                FoodKind::Poison => (IconSlot::Poison, ColorSlot::Poison),
                            };
                            (
                                format!("{} ", theme.icon(icon)),
                                theme_color(theme, color),
                                true,
                            )
                        }
                    };
                    let mut style = Style::default().fg(fg).bg(bg);
                    if bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    Span::styled(text, style)
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

#[test]
fn board_widget_draws_exactly_what_the_old_code_did() {
    let rules = Rules {
        width: 30,
        height: 18,
        wall_mode: WallMode::Gaps,
        inner_walls: InnerWalls::Medium,
        food_interval_s: 1,
        ..Rules::default()
    };
    let mut world = World::with_players(rules, 5, 4).unwrap();
    world.advance_clock(Duration::from_secs(25));
    let view = world.view();
    assert!(view.foods.iter().any(|f| f.1 == FoodKind::Poison));

    let mut theme = Theme::default();
    for colors in [theme.colors, [0, 1, 2, 3, 4, 5, 6]] {
        theme.colors = colors;
        let area = Rect::new(3, 2, 60, 18);
        let mut old = Buffer::empty(Rect::new(0, 0, 70, 24));
        let mut new = old.clone();
        Paragraph::new(old_board_lines(&view, &theme)).render(area, &mut old);
        BoardWidget {
            view: &view,
            theme: &theme,
        }
        .render(area, &mut new);
        assert_eq!(old, new);
    }
}
