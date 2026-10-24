use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use snake_core::solo::GameOverReason;

use super::dialog;
use crate::app::App;

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
