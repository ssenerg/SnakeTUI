//! Choosing a game: every game this server has, by name.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use super::{centered_rect, dim};
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App, selected: usize) {
    let [list_area, footer_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(frame.area());
    let games: Vec<_> = app.games().collect();
    let rect = centered_rect(list_area, 36, games.len() as u16 + 4);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(dim())
        .title(" Pick a game ")
        .title_alignment(Alignment::Center);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines = vec![Line::default()];
    for (i, game) in games.iter().enumerate() {
        let (_, color) = game.logo();
        let (marker, style) = if i == selected {
            (
                " ❯ ",
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        } else {
            ("   ", Style::default())
        };
        let open = if Some(game.id()) == app.game() {
            "  playing now"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{}", game.title()), style),
            Span::styled(open, dim()),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let back = if app.game().is_some() {
        "Esc back"
    } else {
        "q quit"
    };
    let status = match &app.online.notice {
        Some(notice) => Line::styled(
            notice.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        None => Line::default(),
    };
    let footer = vec![
        status,
        Line::styled(format!("↑↓ select · Enter play · {back}"), dim()),
    ];
    frame.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        footer_area,
    );
}
