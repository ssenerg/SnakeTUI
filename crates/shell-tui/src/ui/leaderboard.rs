//! The best players by rating, and where this player stands.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};

use super::centered_rect;
use crate::app::App;
use crate::online::{LeaderRow, MyPlace, RANKED_AFTER, rating_text};

pub fn draw(frame: &mut Frame, app: &App, category: usize) {
    let online = &app.online;
    let categories = app.ui().ratings();
    let label = categories[category.min(categories.len() - 1)].label;
    let [body_area, footer_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(frame.area());
    let dim = Style::default().fg(Color::DarkGray);

    let lines = match &online.leaderboard {
        None => vec![
            Line::default(),
            Line::styled("Loading…", dim).centered(),
            Line::default(),
        ],
        Some(view) if view.rows.is_empty() => vec![
            Line::default(),
            Line::from(format!("Nobody has played {RANKED_AFTER} rated games yet.")).centered(),
            Line::styled(format!("{label} is rated."), dim).centered(),
            Line::default(),
        ],
        Some(view) => {
            // As many rows as fit; the player's own place shows below anyway.
            let fit = body_area.height.saturating_sub(3) as usize;
            let mine = match &view.me {
                MyPlace::Ranked(me) => Some(me.place),
                _ => None,
            };
            let mut lines = vec![Line::styled(
                format!(
                    " {:>3}  {:<16}  {:>6}  {:>5}  {:>4}",
                    "#", "Player", "Rating", "Games", "Wins"
                ),
                dim,
            )];
            lines.extend(view.rows.iter().take(fit).map(|row| {
                let style = if Some(row.place) == mine {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::styled(columns(row), style)
            }));
            lines
        }
    };

    // Which game's, and which of its ratings, when there's a choice.
    let mut title = " Leaderboard ".to_string();
    if app.games().count() > 1 {
        title.push_str(&format!("· {} ", app.ui().title()));
    }
    if categories.len() > 1 {
        title.push_str(&format!("· {label} "));
    }
    let rect = centered_rect(body_area, 48, lines.len() as u16 + 2);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(dim)
        .title(title)
        .title_alignment(Alignment::Center);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);

    let status = match (&online.notice, online.leaderboard.as_ref().map(|v| &v.me)) {
        (Some(notice), _) => Line::styled(
            notice.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        (None, None) => Line::default(),
        (None, Some(MyPlace::Guest)) => Line::styled("Sign up to get a rating.", dim),
        (None, Some(MyPlace::Unranked { games_left })) => {
            let s = if *games_left == 1 { "" } else { "s" };
            Line::styled(
                format!("{games_left} more rated game{s} to be ranked."),
                dim,
            )
        }
        (None, Some(MyPlace::Ranked(me))) => Line::styled(
            format!(
                "You: {} · {} · {} game{}",
                ordinal(me.place),
                me.rating,
                me.games,
                if me.games == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::Cyan),
        ),
    };
    let keys = if categories.len() > 1 {
        "←→ rating · Esc back"
    } else {
        "Esc back"
    };
    let footer = vec![status, Line::styled(keys, dim)];
    frame.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        footer_area,
    );
}

fn columns(row: &LeaderRow) -> String {
    format!(
        " {:>3}  {:<16}  {:>6}  {:>5}  {:>4}",
        row.place,
        row.name,
        rating_text(row.rating, row.provisional),
        row.games,
        row.wins
    )
}

/// 1st, 2nd, 3rd, 4th … 11th, 12th, 13th … 21st.
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::ordinal;

    #[test]
    fn places_read_as_ordinals() {
        let places = [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111, 112];
        assert_eq!(
            places.map(ordinal),
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd",
                "101st", "111th", "112th"
            ]
        );
    }
}
