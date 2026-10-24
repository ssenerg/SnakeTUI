use ratatui::Frame;
use ratatui::layout::{Alignment, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use snake_core::solo::SoloGame;

use super::board::BoardWidget;
use super::centered_rect;
use crate::app::App;

const PAD_X: u16 = 2;
const PAD_Y: u16 = 1;

/// Terminal size a `width`×`height` board needs: two columns per cell, plus
/// border and padding.
fn board_size(width: u16, height: u16) -> (u16, u16) {
    (width * 2 + 2 + 2 * PAD_X, height + 2 + 2 * PAD_Y)
}

/// Whether the board is drawn at all; below this size only a warning is shown.
pub fn board_fits(width: u16, height: u16, size: Size) -> bool {
    let (need_w, need_h) = board_size(width, height);
    size.width >= need_w && size.height >= need_h
}

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(game) = &app.game else { return };
    let area = frame.area();

    let (width, height) = (game.world.width, game.world.height);
    let (need_w, need_h) = board_size(width, height);
    if !board_fits(width, height, area.as_size()) {
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
    let view = game.world.view();
    frame.render_widget(BoardWidget::new(&view, &app.cfg.theme), inner);
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

fn stats_line(app: &App, game: &SoloGame, board_w: u16) -> Line<'static> {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let len = game.len();
    let record = app.record.max(len as u32);
    let secs = game.clock().as_secs();
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

fn hint_line(game: &SoloGame) -> Line<'static> {
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
