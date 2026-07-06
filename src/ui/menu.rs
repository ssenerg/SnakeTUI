use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::centered_rect;
use crate::app::App;
use crate::config::{SECTIONS, SETTINGS};

const LOGO: [&str; 3] = ["┌─┐┌┐┌┌─┐┬┌─┌─┐", "└─┐│││├─┤├┴┐├┤ ", "└─┘┘└┘┴ ┴┴ ┴└─┘"];

enum Row {
    Header(&'static str),
    Item(usize),
    Blank,
}

fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (si, &(title, start)) in SECTIONS.iter().enumerate() {
        if si > 0 {
            rows.push(Row::Blank);
        }
        rows.push(Row::Header(title));
        let end = SECTIONS.get(si + 1).map_or(SETTINGS.len(), |s| s.1);
        for i in start..end {
            rows.push(Row::Item(i));
        }
    }
    rows
}

pub fn draw(frame: &mut Frame, app: &App) {
    let [logo_area, list_area, footer_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(frame.area());

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
    frame.render_widget(Paragraph::new(logo).alignment(Alignment::Center), logo_area);

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
                if let Some(color) = app.cfg.swatch(i) {
                    spans.push(Span::styled("██ ", Style::default().fg(color)));
                }
                if let Some((glyph, color)) = app.cfg.icon_preview(i) {
                    spans.push(Span::styled(
                        format!("{glyph}  "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ));
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

    let record = app.records.get(&app.cfg.record_key());
    let footer = vec![
        Line::from(format!("★ best for this setup: {record}"))
            .style(Style::default().fg(Color::Cyan)),
        Line::from("↑↓ select · ←→ adjust · Enter play · d defaults · q quit")
            .style(Style::default().fg(Color::DarkGray)),
    ];
    frame.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        footer_area,
    );
}
