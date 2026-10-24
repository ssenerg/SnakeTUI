use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use snake_core::settings::{Kind, SETTINGS};

use super::centered_rect;
use crate::app::App;
use crate::palette::theme_color;

const LOGO: [&str; 3] = ["┌─┐┌┐┌┌─┐┬┌─┌─┐", "└─┐│││├─┤├┴┐├┤ ", "└─┘┘└┘┴ ┴┴ ┴└─┘"];

enum Row {
    Header(&'static str),
    Item(usize),
    Blank,
}

fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    let mut section = None;
    for (i, setting) in SETTINGS.iter().enumerate() {
        if section != Some(setting.section) {
            if section.is_some() {
                rows.push(Row::Blank);
            }
            rows.push(Row::Header(setting.section.title()));
            section = Some(setting.section);
        }
        rows.push(Row::Item(i));
    }
    rows
}

pub(super) fn draw_logo(frame: &mut Frame, area: Rect) {
    let logo: Vec<Line> = LOGO
        .iter()
        .map(|l| {
            Line::from(*l).style(
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(logo).alignment(Alignment::Center), area);
}

pub fn draw(frame: &mut Frame, app: &App) {
    let [logo_area, list_area, footer_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    draw_logo(frame, logo_area);

    let rect = centered_rect(list_area, 58.min(list_area.width), list_area.height);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Settings ")
        .title_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .title_alignment(Alignment::Center);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let rows = rows();
    let sel_row = rows
        .iter()
        .position(|r| matches!(r, Row::Item(i) if *i == app.selected))
        .unwrap_or(0);
    let visible = inner.height as usize;
    let offset = if visible == 0 || sel_row < visible {
        0
    } else {
        sel_row + 1 - visible
    };

    let mut lines = Vec::new();
    for row in rows.iter().skip(offset).take(visible.max(1)) {
        lines.push(match row {
            Row::Blank => Line::default(),
            Row::Header(title) => Line::from(vec![
                Span::styled(" ── ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    (*title).to_string(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        " {}",
                        "─".repeat((inner.width as usize).saturating_sub(title.len() + 6))
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            &Row::Item(i) => {
                let selected = i == app.selected;
                let label_style = if selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let marker = if selected { " ❯ " } else { "   " };
                let mut spans = vec![Span::styled(
                    format!("{marker}{:<26}", SETTINGS[i].label),
                    label_style,
                )];
                let theme = &app.cfg.theme;
                match SETTINGS[i].kind {
                    Kind::Color(slot) => {
                        let color = theme_color(theme, slot);
                        spans.push(Span::styled("██ ", Style::default().fg(color)));
                    }
                    // The icon as it appears in game, in its in-game color.
                    Kind::Icon(slot) => {
                        let color = theme_color(theme, slot.color_slot());
                        spans.push(Span::styled(
                            format!("{}  ", theme.icon(slot)),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ));
                    }
                    _ => {}
                }
                let value = app.cfg.value_str(i);
                if selected {
                    spans.push(Span::styled(format!("◂ {value} ▸"), label_style));
                } else {
                    spans.push(Span::styled(
                        format!("  {value}"),
                        Style::default().fg(Color::Gray),
                    ));
                }
                Line::from(spans)
            }
        });
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let record = app.records.get(&app.cfg.rules.record_key());
    let hint = if app.confirm_reset {
        Line::from("press r again to reset all settings · any other key cancels").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        let back = if app.online.is_some() { "back" } else { "quit" };
        Line::from(format!(
            "↑↓ select · ←→ adjust · Enter play · r reset · q {back}"
        ))
        .style(Style::default().fg(Color::DarkGray))
    };
    let footer = vec![
        Line::from(format!("★ best for this setup: {record}"))
            .style(Style::default().fg(Color::Cyan)),
        hint,
    ];
    frame.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        footer_area,
    );
}
