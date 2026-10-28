//! Friends, and friend requests both ways.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use super::centered_rect;
use crate::app::App;
use crate::online::FriendsRow;

pub fn draw(frame: &mut Frame, app: &App, selected: usize) {
    let online = &app.online;
    let view = &online.friends;
    let rows = view.rows();
    let [body_area, footer_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(frame.area());

    // Headings above each kind of row.
    let headings = usize::from(!view.incoming.is_empty())
        + usize::from(!view.friends.is_empty())
        + usize::from(!view.outgoing.is_empty());
    let height = (rows.len() + headings * 2 + 3) as u16;
    let rect = centered_rect(body_area, 64, height);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Friends ")
        .title_alignment(Alignment::Center);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let dim = Style::default().fg(Color::DarkGray);
    let heading = |text: &'static str| Line::styled(format!("  {text}"), dim);
    let mut lines = vec![Line::default()];
    let mut last = None;
    for (i, row) in rows.iter().enumerate() {
        let active = i == selected;
        let style = if active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let marker = if active { "  ❯ " } else { "    " };
        let kind = std::mem::discriminant(row);
        if last != Some(kind) {
            match row {
                FriendsRow::Add => {}
                FriendsRow::Request(_) => {
                    lines.push(Line::default());
                    lines.push(heading("Want to be your friends"));
                }
                FriendsRow::Friend(_) => {
                    lines.push(Line::default());
                    lines.push(heading("Friends"));
                }
                FriendsRow::Asked(_) => {
                    lines.push(Line::default());
                    lines.push(heading("Waiting for an answer"));
                }
            }
            last = Some(kind);
        }
        lines.push(match row {
            FriendsRow::Add => Line::styled(format!("{marker}Add a friend"), style),
            FriendsRow::Request(r) => Line::from(vec![
                Span::styled(format!("{marker}{:<16}", view.incoming[*r]), style),
                Span::styled("  a accept · d decline", dim),
            ]),
            FriendsRow::Friend(f) => {
                let friend = &view.friends[*f];
                let status = if friend.online {
                    Style::default().fg(Color::LightGreen)
                } else {
                    dim
                };
                let mut spans = vec![
                    Span::styled(format!("{marker}{:<16}", friend.name), style),
                    Span::styled(format!("  {}", friend.status), status),
                ];
                if friend.watch {
                    spans.push(Span::styled(" · v watch", dim));
                }
                Line::from(spans)
            }
            FriendsRow::Asked(a) => Line::from(vec![
                Span::styled(format!("{marker}{:<16}", view.outgoing[*a]), style),
                Span::styled("  x take back", dim),
            ]),
        });
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let status = match &online.notice {
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
        Line::styled(
            "↑↓ select · Enter invite or answer · v watch · x remove · Esc back",
            dim,
        ),
    ];
    frame.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        footer_area,
    );
}
