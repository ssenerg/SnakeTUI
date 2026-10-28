use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use super::{centered_rect, draw_logo};
use crate::app::App;
use crate::online::HomeItem;

pub fn draw(frame: &mut Frame, app: &App, selected: usize) {
    let online = &app.online;
    let items = app.home_items();
    let [logo_area, list_area, footer_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(3),
        Constraint::Length(4),
    ])
    .areas(frame.area());
    draw_logo(frame, app, logo_area);

    let rect = centered_rect(list_area, 44, items.len() as u16 + 4);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let dim = Style::default().fg(Color::DarkGray);
    let mut lines = vec![Line::default()];
    for (i, item) in items.iter().enumerate() {
        let active = i == selected;
        let style = if active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let marker = if active { " ❯ " } else { "   " };
        let mut spans = vec![Span::styled(format!("{marker}{}", app.label(*item)), style)];
        match item {
            HomeItem::Nickname => spans.push(Span::styled(format!("  {}", online.name), dim)),
            HomeItem::CreateRoom => {
                if let Some(note) = app.ui().room_note() {
                    spans.push(Span::styled(format!("  {note}"), dim));
                }
            }
            HomeItem::Friends if online.requests > 0 => {
                let s = if online.requests == 1 { "" } else { "s" };
                spans.push(Span::styled(
                    format!("  {} request{s}", online.requests),
                    Style::default().fg(Color::LightYellow),
                ));
            }
            HomeItem::Account => {
                if let Some(shown) = app.main_rating() {
                    spans.push(Span::styled(format!("  rating {}", shown.text()), dim));
                }
            }
            _ => {}
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let name = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let status = match (&online.notice, &online.account) {
        (Some(notice), _) => Line::from(notice.as_str()).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        (None, Some(profile)) => {
            let mut spans = vec![
                Span::styled("Signed in as ", dim),
                Span::styled(profile.username.clone(), name),
            ];
            if let Some(shown) = app.main_rating() {
                spans.push(Span::styled(format!(" · rating {}", shown.text()), dim));
            }
            Line::from(spans)
        }
        (None, None) => Line::from(vec![
            Span::styled("Playing as ", dim),
            Span::styled(online.name.clone(), name),
            Span::styled(" (guest)", dim),
        ]),
    };
    let keys = if app.games().count() > 1 {
        "↑↓ select · Enter choose · g games · q quit"
    } else {
        "↑↓ select · Enter choose · q quit"
    };
    let footer = vec![status, Line::from(keys).style(dim)];
    frame.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        footer_area,
    );
}
