use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use super::centered_rect;
use super::menu::draw_logo;
use crate::app::{App, HOME, HomeItem};

pub fn draw(frame: &mut Frame, app: &App, selected: usize) {
    let Some(online) = &app.online else { return };
    let [logo_area, list_area, footer_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(3),
        Constraint::Length(4),
    ])
    .areas(frame.area());
    draw_logo(frame, logo_area);

    let rect = centered_rect(list_area, 44, HOME.len() as u16 + 4);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines = vec![Line::default()];
    for (i, item) in HOME.iter().enumerate() {
        let active = i == selected;
        let style = if active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let marker = if active { " ❯ " } else { "   " };
        let mut spans = vec![Span::styled(format!("{marker}{}", item.label()), style)];
        let dim = Style::default().fg(Color::DarkGray);
        match item {
            HomeItem::Nickname => spans.push(Span::styled(format!("  {}", online.name), dim)),
            HomeItem::CreateRoom => spans.push(Span::styled("  your settings", dim)),
            _ => {}
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let status = match &online.notice {
        Some(notice) => Line::from(notice.as_str()).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        None => Line::from(vec![
            Span::styled("Playing as ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                online.name.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    };
    let footer = vec![
        status,
        Line::from("↑↓ select · Enter choose · q quit").style(Style::default().fg(Color::DarkGray)),
    ];
    frame.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        footer_area,
    );
}
