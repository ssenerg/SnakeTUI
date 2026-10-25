use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use snake_core::solo::GameOverReason;

use super::dialog;
use crate::app::App;
use crate::online::{BOTS_OFFERED_AFTER, Field};

pub fn draw_pause(frame: &mut Frame, selected: usize) {
    let inner = dialog(frame, " PAUSED ", Color::Yellow, 36, 8);

    let item = |label: &str, active: bool| {
        if active {
            Line::from(format!("❯ {label}")).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Line::from(format!("  {label}"))
        }
    };
    let lines = vec![
        Line::default(),
        item("Resume", selected == 0),
        item("Quit to menu", selected == 1),
        Line::default(),
        Line::from("↑↓ + Enter · Esc resume · q menu").style(Style::default().fg(Color::DarkGray)),
    ];
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

pub fn draw_game_over(frame: &mut Frame, app: &App, new_record: bool) {
    let Some(game) = &app.game else { return };
    let (reason, color) = match game.over {
        Some(GameOverReason::HitWall) => ("You crashed into a wall!", Color::LightRed),
        Some(GameOverReason::HitSelf) => ("You bit yourself!", Color::LightRed),
        Some(GameOverReason::Poisoned) => ("✖ You ate poison!", Color::LightMagenta),
        Some(GameOverReason::MaxLength) => ("★ Board complete — you win! ★", Color::LightGreen),
        None => ("", Color::White),
    };
    let title = if matches!(game.over, Some(GameOverReason::MaxLength)) {
        " YOU WIN "
    } else {
        " GAME OVER "
    };
    let inner = dialog(frame, title, color, 46, 11);

    let dim = Style::default().fg(Color::DarkGray);
    let secs = game.clock().as_secs();
    let mut lines = vec![
        Line::default(),
        Line::from(reason).style(Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Line::default(),
        Line::from(format!(
            "Length {}    Time {:02}:{:02}",
            game.len(),
            secs / 60,
            secs % 60
        )),
        Line::from(format!("★ Best {}", app.record)),
    ];
    if new_record {
        lines.push(
            Line::from("★ NEW RECORD ★").style(
                Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::BOLD),
            ),
        );
    }
    lines.push(Line::default());
    lines.push(Line::from("r play again · Enter menu · q quit").style(dim));
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

pub fn draw_input(frame: &mut Frame, app: &App, field: Field) {
    let (title, prompt) = match field {
        Field::Code => (" Join with a code ", "Type the room's 6-character code"),
        Field::Nickname => (" Nickname ", "Letters, digits, _ and -, up to 16"),
        Field::FriendName => (" Add a friend ", "Their username"),
    };
    let inner = dialog(frame, title, Color::Cyan, 44, 9);
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines = vec![
        Line::default(),
        Line::from(prompt).style(dim),
        Line::default(),
        Line::from(format!("{}▏", app.input.text)).style(
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    lines.push(match &app.input.error {
        Some(error) => Line::from(error.as_str()).style(Style::default().fg(Color::LightRed)),
        None => Line::default(),
    });
    lines.push(Line::from("Enter ok · Esc cancel").style(dim));
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

pub fn draw_finding(frame: &mut Frame, app: &App) {
    let Some(online) = &app.online else { return };
    let secs = online.waited.as_secs();
    let mut status = format!(
        "{}:{:02} · {} in queue",
        secs / 60,
        secs % 60,
        online.queue_len.max(1)
    );
    if let Some(region) = &online.region {
        let name: String = region.name.chars().take(12).collect();
        status = format!("{name} · {status}");
        if online.waited >= region.widens_after {
            status.push_str(" · all regions");
        }
    }
    // Nobody yet: bots are offered instead.
    let bots = online.waited >= BOTS_OFFERED_AFTER;
    let width = if online.region.is_some() || bots {
        50
    } else {
        36
    };
    let height = if bots { 9 } else { 7 };
    let inner = dialog(frame, " Finding a match ", Color::Cyan, width, height);
    let mut lines = vec![Line::default(), Line::from(status), Line::default()];
    if bots {
        lines.push(Line::from("Nobody yet? Play bots now, unrated."));
        lines.push(Line::default());
        lines.push(
            Line::from("b play bots · Esc cancel").style(Style::default().fg(Color::DarkGray)),
        );
    } else {
        lines.push(Line::from("Esc cancel").style(Style::default().fg(Color::DarkGray)));
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

pub fn draw_joining(frame: &mut Frame) {
    let inner = dialog(frame, " Joining ", Color::Cyan, 30, 5);
    let lines = vec![Line::default(), Line::from("One moment…")];
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}
