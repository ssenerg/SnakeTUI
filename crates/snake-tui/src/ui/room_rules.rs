//! A private room's rules, from its lobby: everyone reads them, the host
//! changes them.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use snake_core::config::GameConfig;
use snake_core::settings::{SETTINGS, room_settings};

use super::centered_rect;
use super::menu::draw_settings;
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App, selected: usize) {
    let Some(online) = &app.online else { return };
    let Some(room) = &online.room else { return };
    let [body_area, footer_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(frame.area());

    let items = room_settings();
    // Every row, and each section's heading with a blank line between.
    let sections = 1 + items
        .windows(2)
        .filter(|w| SETTINGS[w[0]].section != SETTINGS[w[1]].section)
        .count();
    let rows = items.len() + sections * 2 - 1;
    let rect = centered_rect(body_area, 58, rows as u16 + 2);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Room rules ")
        .title_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .title_alignment(Alignment::Center);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let cfg = GameConfig {
        rules: room.view.settings.clone(),
        ..app.cfg.clone()
    };
    let hosting = room.hosting();
    let selected = items[selected.min(items.len() - 1)];
    draw_settings(frame, inner, &cfg, &items, selected, hosting);

    let dim = Style::default().fg(Color::DarkGray);
    let status = match (&online.notice, hosting) {
        (Some(notice), _) => Line::styled(
            notice.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        (None, true) => Line::styled(
            format!(
                "Room for {} players · a change un-readies everyone",
                room.view.shell.max_players
            ),
            dim,
        ),
        (None, false) => {
            let host = room
                .view
                .shell
                .host()
                .map_or("the host", |h| h.who.name.as_str());
            Line::styled(format!("Only {host} can change these."), dim)
        }
    };
    let hint = if hosting {
        "↑↓ select · ←→ change · Esc back"
    } else {
        "↑↓ scroll · Esc back"
    };
    frame.render_widget(
        Paragraph::new(vec![status, Line::styled(hint, dim)])
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        footer_area,
    );
}
