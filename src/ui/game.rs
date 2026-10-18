use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Alignment, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};

use super::centered_rect;
use crate::app::App;
use crate::config::{ColorSlot, IconSlot};
use crate::game::{FoodKind, Game};

const PAD_X: u16 = 2;
const PAD_Y: u16 = 1;

/// Terminal size the board needs: two columns per cell, plus border and padding.
fn board_size(game: &Game) -> (u16, u16) {
    (game.width * 2 + 2 + 2 * PAD_X, game.height + 2 + 2 * PAD_Y)
}

/// Whether the board is drawn at all; below this size only a warning is shown.
pub fn board_fits(game: &Game, size: Size) -> bool {
    let (need_w, need_h) = board_size(game);
    size.width >= need_w && size.height >= need_h
}

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(game) = &app.game else { return };
    let area = frame.area();

    let (need_w, need_h) = board_size(game);
    if !board_fits(game, area.as_size()) {
        frame.render_widget(
            Paragraph::new(format!(
                "terminal too small — need at least {need_w}x{need_h}"
            ))
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Red)),
            area,
        );
        return;
    }

    let boost = game.boost_active();
    let border = if boost {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let rect = centered_rect(area, need_w, need_h);
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(border))
        .padding(Padding::new(PAD_X, PAD_X, PAD_Y, PAD_Y))
        .title_top(title_line())
        .title_top(stats_line(app, game, need_w).right_aligned())
        .title_bottom(hint_line(game).centered());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(board_lines(game)), inner);
}

fn title_line() -> Line<'static> {
    Line::styled(
        " S N A K E ",
        Style::default()
            .fg(Color::LightGreen)
            .add_modifier(Modifier::BOLD),
    )
    .left_aligned()
}

fn stats_line(app: &App, game: &Game, board_w: u16) -> Line<'static> {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let len = game.len();
    let record = app.record.max(len as u32);
    let secs = game.clock.as_secs();
    let time = format!("{:02}:{:02}", secs / 60, secs % 60);

    if board_w < 52 {
        return Line::from(vec![
            Span::styled(format!(" {len} "), bold),
            Span::styled("★", Style::default().fg(Color::Yellow)),
            Span::styled(format!("{record} "), bold),
            Span::styled(format!("{time} "), bold),
        ]);
    }
    Line::from(vec![
        Span::styled(" Length ", dim),
        Span::styled(len.to_string(), bold),
        Span::styled(" · ", dim),
        Span::styled("★ ", Style::default().fg(Color::Yellow)),
        Span::styled("Best ", dim),
        Span::styled(record.to_string(), bold),
        Span::styled(" · ", dim),
        Span::styled(time, bold),
        Span::raw(" "),
    ])
}

fn hint_line(game: &Game) -> Line<'static> {
    if !game.started {
        Line::styled(
            " press an arrow key to start ",
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if game.boost_active() {
        Line::styled(
            " ⚡ BOOST ×2 ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Line::styled(
            " ←↑↓→ move · Esc pause ",
            Style::default().fg(Color::DarkGray),
        )
    }
}

fn board_lines(game: &Game) -> Vec<Line<'static>> {
    #[derive(Clone, Copy)]
    enum Cell {
        Empty,
        Wall,
        Food,
        Super,
        Poison,
        Body,
        Head,
    }

    let cfg = &game.cfg;
    let (w, h) = (game.width as usize, game.height as usize);
    let mut grid = vec![Cell::Empty; w * h];
    for y in 0..game.height {
        for x in 0..game.width {
            if game.wall_at(x, y) {
                grid[y as usize * w + x as usize] = Cell::Wall;
            }
        }
    }
    for food in &game.foods {
        grid[food.pos.y as usize * w + food.pos.x as usize] = match food.kind {
            FoodKind::Normal => Cell::Food,
            FoodKind::Super => Cell::Super,
            FoodKind::Poison => Cell::Poison,
        };
    }
    for p in game.snake.iter().skip(1) {
        grid[p.y as usize * w + p.x as usize] = Cell::Body;
    }
    if let Some(head) = game.snake.front() {
        grid[head.y as usize * w + head.x as usize] = Cell::Head;
    }

    // Each food cell is the user-chosen glyph padded to the 2-column cell.
    let food_txt = format!("{} ", cfg.icon(IconSlot::Food));
    let super_txt = format!("{} ", cfg.icon(IconSlot::SuperFood));
    let poison_txt = format!("{} ", cfg.icon(IconSlot::Poison));

    let bg = cfg.color(ColorSlot::Background);
    let mut lines = Vec::with_capacity(h);
    for y in 0..h {
        let mut spans = Vec::with_capacity(w);
        for x in 0..w {
            let (text, fg, bold): (Cow<'static, str>, _, _) = match grid[y * w + x] {
                Cell::Empty => ("  ".into(), bg, false),
                Cell::Wall => ("██".into(), cfg.color(ColorSlot::Wall), false),
                Cell::Head => ("██".into(), cfg.color(ColorSlot::Head), false),
                Cell::Body => ("██".into(), cfg.color(ColorSlot::Body), false),
                Cell::Food => (food_txt.clone().into(), cfg.color(ColorSlot::Food), true),
                Cell::Super => (
                    super_txt.clone().into(),
                    cfg.color(ColorSlot::SuperFood),
                    true,
                ),
                Cell::Poison => (
                    poison_txt.clone().into(),
                    cfg.color(ColorSlot::Poison),
                    true,
                ),
            };
            let mut style = Style::default().fg(fg).bg(bg);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(text, style));
        }
        lines.push(Line::from(spans));
    }
    lines
}
