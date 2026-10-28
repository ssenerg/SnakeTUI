//! A room, of whichever game: its lobby, and its results over the game's
//! drawing of the round.

use std::time::Duration;

use game_api::{PhaseView, RoomKind, is_bot};
use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{dialog, dim, named, ping_style, ping_text, secs};
use crate::app::App;
use crate::game::GameUi;
use crate::online::InRoom;

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(room) = &app.online.room else {
        return;
    };
    let Some(ui) = app.games().find(|g| g.id() == room.view.game()) else {
        return;
    };
    match &room.shell().phase {
        PhaseView::Waiting { starts_in } => draw_lobby(frame, app, ui, room, *starts_in),
        phase => {
            ui.draw_round(frame, room, &app.ctx());
            if let PhaseView::Results { next_in } = phase {
                draw_results(frame, ui, room, *next_in);
            }
        }
    }
}

fn draw_lobby(
    frame: &mut Frame,
    app: &App,
    ui: &dyn GameUi,
    room: &InRoom,
    starts_in: Option<Duration>,
) {
    let view = room.shell();
    let hosting = room.hosting();
    let watchers = u16::from(view.watchers > 0);
    let height = 12 + view.seats.len() as u16 + u16::from(hosting) + watchers;
    let inner = dialog(frame, " Private room ", Color::Cyan, 60, height);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::default()];
    if let Some(code) = &view.code {
        lines.push(Line::from(vec![
            Span::styled("Code ", dim()),
            Span::styled(code.clone(), bold.fg(Color::LightYellow)),
        ]));
        lines.push(Line::from("Share it so friends can join.").style(dim()));
    }
    // The game's settings, then how full the room is.
    let mut about = ui.lobby_line(room);
    let players = format!("{}/{} players", view.seats.len(), view.max_players);
    if about.is_empty() {
        about.push(Span::styled(players, dim()));
    } else {
        about.push(Span::styled(format!(" · {players}"), dim()));
    }
    lines.push(Line::from(about));
    lines.push(Line::default());
    for seat in &view.seats {
        let (mark, style) = if seat.ready {
            ("✓ ", Style::default().fg(Color::LightGreen))
        } else {
            ("· ", dim())
        };
        let mut spans = vec![
            Span::styled(mark, style),
            Span::styled(seat.who.name.clone(), bold),
        ];
        if seat.who.id == room.me {
            spans.push(Span::styled(" (you)", dim()));
        }
        if is_bot(seat.who.id) {
            spans.push(Span::styled(" (bot)", dim()));
        }
        if seat.wins > 0 {
            let s = if seat.wins == 1 { "" } else { "s" };
            spans.push(Span::styled(format!(" · {} win{s}", seat.wins), dim()));
        }
        if let Some(ping) = seat.ping {
            spans.push(Span::styled(" · ", dim()));
            spans.push(Span::styled(ping_text(ping, false), ping_style(ping)));
        }
        lines.push(Line::from(spans));
    }
    if view.watchers > 0 {
        lines.push(Line::styled(format!("{} watching", view.watchers), dim()));
    }
    lines.push(Line::default());
    lines.push(match starts_in {
        Some(left) => Line::from(format!(
            "Starting in {} s, or when everyone is ready",
            secs(left)
        )),
        None if view.seats.len() < 2 => Line::from("Waiting for another player…"),
        None => Line::default(),
    });
    lines.push(match &app.online.notice {
        Some(notice) => Line::styled(notice.clone(), Style::default().fg(Color::LightYellow)),
        None => Line::default(),
    });
    let signed_in = app.online.account.is_some();
    let mut keys = Vec::new();
    match &room.watching {
        Some(friend) => keys.push(format!("Watching {friend}")),
        None => keys.push("Enter ready".to_string()),
    }
    keys.extend(ui.lobby_keys().map(str::to_string));
    match (&room.watching, hosting, signed_in) {
        (None, true, true) => keys.push("i invite".into()),
        (None, false, true) => keys.push("i invite friends".into()),
        _ => {}
    }
    keys.push("Esc leave".into());
    lines.push(Line::from(keys.join(" · ")).style(dim()));
    if hosting {
        let mut host = Vec::new();
        host.extend(ui.host_keys());
        if ui.bots() {
            host.push("b add a bot · x take one out");
        }
        lines.push(Line::from(host.join(" · ")).style(dim()));
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn draw_results(frame: &mut Frame, ui: &dyn GameUi, room: &InRoom, next_in: Option<Duration>) {
    let view = room.shell();
    let height = 9 + view.placements.len() as u16;
    let (headline, color) = match view.winner() {
        Some(winner) if winner.id == room.me => ("★ You win! ★".to_string(), Color::LightGreen),
        Some(winner) => (format!("{} wins!", named(winner)), Color::LightCyan),
        None => ("Draw!".to_string(), Color::LightYellow),
    };
    let inner = dialog(frame, " RESULTS ", color, 48, height);
    let mut lines = vec![
        Line::default(),
        Line::styled(
            headline,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        match ui.result_note(room) {
            Some(note) => Line::styled(note, dim()),
            None => Line::default(),
        },
    ];
    let followed = room.followed().map(|s| s.who.id);
    for p in &view.placements {
        let style = if Some(p.who.id) == followed {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let name: String = named(&p.who).chars().take(18).collect();
        let score: String = ui.score(room, p.who.id).chars().take(4).collect();
        let mut spans = vec![Span::styled(
            format!("{:>2}. {name:<18} {score:>4}", p.place),
            style,
        )];
        // Rated matches: how the round moved each account's rating.
        match view.ratings.iter().find(|r| r.id == p.who.id) {
            Some(r) if r.after >= r.before => spans.push(Span::styled(
                format!("  +{:<3}", r.after - r.before),
                Style::default().fg(Color::LightGreen),
            )),
            Some(r) => spans.push(Span::styled(
                format!("  −{:<3}", r.before - r.after),
                Style::default().fg(Color::LightRed),
            )),
            None => spans.push(Span::raw("      ")),
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::default());
    lines.push(match (view.kind, next_in) {
        _ if room.watching.is_some() => Line::styled("Esc leave", dim()),
        (RoomKind::Match, _) => Line::styled("Enter find another match · Esc home", dim()),
        (RoomKind::Private, Some(left)) => {
            Line::styled(format!("Next round in {} s · Esc leave", secs(left)), dim())
        }
        (RoomKind::Private, None) => Line::styled("Esc leave", dim()),
    });
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}
