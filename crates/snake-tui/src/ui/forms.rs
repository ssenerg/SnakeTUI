//! Signing up, signing in, and changing a password.

use ratatui::Frame;
use ratatui::layout::{Alignment, Margin};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::dialog;
use crate::app::App;
use crate::online::{FormKind, FormRow};

pub fn draw(frame: &mut Frame, app: &App, kind: FormKind) {
    let has_key = app.online.as_ref().is_some_and(|o| o.has_key);
    let rows = kind.rows(has_key);
    let form = &app.form;
    let inner = dialog(frame, kind.title(), Color::Cyan, 56, rows.len() as u16 + 11)
        .inner(Margin::new(2, 0));
    let dim = Style::default().fg(Color::DarkGray);
    let field = |label: &str, value: String, focused: bool| {
        let (marker, style) = if focused {
            (
                "❯ ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("  ", Style::default())
        };
        let cursor = if focused { "▏" } else { "" };
        Line::from(vec![
            Span::styled(format!("{marker}{label:<17}"), style),
            Span::styled(format!("{value}{cursor}"), style.fg(Color::LightYellow)),
        ])
    };
    let masked = |len: usize| "•".repeat(len);

    let mut lines = vec![Line::default()];
    for (i, row) in rows.iter().enumerate() {
        let focused = i == form.focus;
        lines.push(match row {
            FormRow::Username => field("Username", form.username.clone(), focused),
            FormRow::Password => {
                let label = if kind == FormKind::ChangePassword {
                    "New password"
                } else {
                    "Password"
                };
                field(label, masked(form.password.len()), focused)
            }
            FormRow::Repeat => field("Repeat password", masked(form.repeat.len()), focused),
            FormRow::Remember => {
                let mark = if form.remember { "[x]" } else { "[ ]" };
                field(mark, "Remember this machine".to_string(), focused)
            }
        });
    }
    lines.push(Line::default());
    lines.push(match (&form.error, form.busy) {
        (_, true) => Line::from("One moment…"),
        (Some(error), _) => Line::styled(error.clone(), Style::default().fg(Color::LightRed)),
        (None, _) => Line::default(),
    });
    lines.push(Line::default());
    let note = match kind {
        FormKind::SignUp if has_key => {
            "Pick a password just for SnakeTUI, not your computer's. \
             This machine will sign you in by itself from now on."
        }
        FormKind::SignUp | FormKind::ChangePassword => {
            "Pick a password just for SnakeTUI, not your computer's."
        }
        FormKind::SignIn => "Your SnakeTUI username and password.",
    };
    lines.push(Line::styled(note, dim));
    lines.push(Line::default());
    let hint = if rows.contains(&FormRow::Remember) {
        "↑↓ move · Space tick · Enter next · Esc back"
    } else {
        "↑↓ move · Enter next · Esc back"
    };
    lines.push(Line::styled(hint, dim));
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false }),
        inner,
    );
}
