//! A multiplayer room: the lobby of a private room, and the board with its
//! scoreboard while a round plays.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use snake_core::config::ColorSlot;
use snake_core::geom::Point;
use snake_core::round::{Entrant, Outcome, PhaseView, RoomKind, is_bot};
use snake_core::view::MatchView;

use super::board::{BoardWidget, SnakeLook};
use super::dialog;
use crate::app::App;
use crate::online::InRoom;
use crate::palette::{opponent_colors, theme_color};

/// Columns the scoreboard takes beside the board.
const SCORE_W: u16 = 18;

/// Terminal size that shows a `width`×`height` room board whole: the board
/// and its border, the scoreboard beside it, and a status line below.
pub fn room_size(width: u16, height: u16) -> (u16, u16) {
    (width * 2 + 2 + SCORE_W, height + 2 + 1)
}

pub fn room_fits(width: u16, height: u16, size: Size) -> bool {
    let (w, h) = room_size(width, height);
    size.width >= w && size.height >= h
}

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(room) = app.online.as_ref().and_then(|o| o.room.as_ref()) else {
        return;
    };
    match (&room.view.phase, &room.view.board) {
        (PhaseView::Waiting { starts_in }, _) => draw_lobby(frame, app, room, *starts_in),
        (_, Some(board)) => draw_match(frame, app, room, board),
        _ => {}
    }
}

/// A player's name, marked if they're a bot.
fn named(who: &Entrant) -> String {
    if is_bot(who.id) {
        format!("{} (bot)", who.name)
    } else {
        who.name.clone()
    }
}

fn secs(d: Duration) -> u64 {
    d.as_secs_f64().ceil() as u64
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// A round trip as shown: "42 ms", or "1.2 s" from a second on. `tight`
/// leaves out the space.
fn ping_text(ping: Duration, tight: bool) -> String {
    let gap = if tight { "" } else { " " };
    match ping.as_millis() {
        ms @ 0..1000 => format!("{}{gap}ms", ms.max(1)),
        _ => format!("{:.1}{gap}s", ping.as_secs_f64()),
    }
}

/// Green while turns feel instant, yellow once they lag, red when they lag
/// badly.
fn ping_style(ping: Duration) -> Style {
    let color = match ping.as_millis() {
        0..80 => Color::LightGreen,
        80..200 => Color::Yellow,
        _ => Color::LightRed,
    };
    Style::default().fg(color)
}

fn draw_lobby(frame: &mut Frame, app: &App, room: &InRoom, starts_in: Option<Duration>) {
    let view = &room.view;
    let online = app.online.as_ref();
    let watchers = u16::from(view.watchers > 0);
    let height = 12 + view.seats.len() as u16 + u16::from(room.hosting()) + watchers;
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
    // The host can change the speed, so theirs stands out, with arrows.
    let tick = format!("{} ms ticks", view.rules.tick_ms);
    let tick = if room.hosting() {
        Span::styled(
            format!("◂ {tick} ▸"),
            Style::default().fg(Color::LightYellow),
        )
    } else {
        Span::styled(tick, dim())
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}×{} board · ", view.rules.width, view.rules.height),
            dim(),
        ),
        tick,
        Span::styled(
            format!(" · {}/{} players", view.seats.len(), view.max_players),
            dim(),
        ),
    ]));
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
    lines.push(match online.and_then(|o| o.notice.as_ref()) {
        Some(notice) => Line::styled(notice.clone(), Style::default().fg(Color::LightYellow)),
        None => Line::default(),
    });
    let signed_in = online.is_some_and(|o| o.account.is_some());
    let hint = match (&room.watching, room.hosting(), signed_in) {
        (Some(friend), _, _) => format!("Watching {friend} · e rules · Esc leave"),
        (None, true, true) => "Enter ready · e rules · i invite · Esc leave".into(),
        (None, false, true) => "Enter ready · e rules · i invite friends · Esc leave".into(),
        (None, _, false) => "Enter ready · e rules · Esc leave".into(),
    };
    lines.push(Line::from(hint).style(dim()));
    if room.hosting() {
        lines.push(Line::from("←→ speed · b add a bot · x take one out").style(dim()));
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

/// How each snake on `board` is drawn for this player: their own in their
/// theme's colors, the others in colors that stand apart from it.
fn looks(app: &App, room: &InRoom, board: &MatchView) -> Vec<SnakeLook> {
    let theme = &app.cfg.theme;
    let mine = room.followed().and_then(|s| s.slot);
    let others = opponent_colors(theme);
    let count = board
        .snakes
        .iter()
        .map(|s| s.id as usize + 1)
        .max()
        .unwrap_or(0);
    let mut looks = vec![
        SnakeLook {
            head: Color::White,
            body: Color::White,
            number: None,
        };
        count
    ];
    let mut next = 0;
    for snake in &board.snakes {
        let number = Some(snake.id + 1);
        looks[snake.id as usize] = if Some(snake.id) == mine {
            SnakeLook {
                head: theme_color(theme, ColorSlot::Head),
                body: theme_color(theme, ColorSlot::Body),
                number,
            }
        } else {
            let color = others[next % others.len()];
            next += 1;
            SnakeLook {
                head: color,
                body: color,
                number,
            }
        };
    }
    looks
}

/// The board cell to draw at the top-left, so `center` stays in view when
/// only `visible` cells of the board fit.
fn origin(center: Point, board: &MatchView, visible: (u16, u16)) -> Point {
    let axis = |c: u16, size: u16, vis: u16| {
        if vis >= size {
            0
        } else {
            c.saturating_sub(vis / 2).min(size - vis)
        }
    };
    Point {
        x: axis(center.x, board.width, visible.0),
        y: axis(center.y, board.height, visible.1),
    }
}

fn draw_match(frame: &mut Frame, app: &App, room: &InRoom, board: &MatchView) {
    let area = frame.area();
    let full_w = board.width * 2 + 2;
    let full_h = board.height + 2;
    let (board_w, score_w) = if area.width >= full_w + SCORE_W {
        (full_w, (area.width - full_w).min(30))
    } else if area.width >= SCORE_W + 12 {
        (area.width - SCORE_W, SCORE_W)
    } else {
        (area.width, 0)
    };
    // The board, with the status line right under it.
    let board_h = full_h.min(area.height.saturating_sub(1));
    let x0 = area.x + (area.width - board_w - score_w) / 2;
    let y0 = area.y + (area.height - (board_h + 1).min(area.height)) / 2;
    let board_rect = Rect::new(x0, y0, board_w, board_h);
    let score_rect = Rect::new(x0 + board_w, y0, score_w, board_h);
    let status = Rect::new(area.x, y0 + board_h, area.width, 1).intersection(area);
    let cropped = board_w < full_w || board_h < full_h;

    let clock = match &room.view.phase {
        PhaseView::Running { left } => {
            let s = secs(*left);
            format!(" {:02}:{:02} ", s / 60, s % 60)
        }
        _ => String::new(),
    };
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(dim())
        .title_top(
            Line::styled(
                " S N A K E ",
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            )
            .left_aligned(),
        )
        .title_top(Line::from(clock).right_aligned());
    let inner = block.inner(board_rect);
    frame.render_widget(block, board_rect);

    let looks = looks(app, room, board);
    let me = room
        .followed()
        .and_then(|s| s.slot)
        .and_then(|slot| board.snakes.iter().find(|s| s.id == slot))
        .and_then(|s| s.body.first().copied());
    let center = me.unwrap_or(Point {
        x: board.width / 2,
        y: board.height / 2,
    });
    let widget = BoardWidget {
        view: board,
        theme: &app.cfg.theme,
        origin: origin(center, board, (inner.width / 2, inner.height)),
        looks: Some(&looks),
    };
    frame.render_widget(widget, inner);

    if score_w > 0 {
        draw_scores(frame, room, &looks, score_rect);
    }
    let (need_w, need_h) = room_size(board.width, board.height);
    frame.render_widget(
        Paragraph::new(status_line(room, cropped, need_w, need_h)).alignment(Alignment::Center),
        status,
    );
    match &room.view.phase {
        PhaseView::Countdown { left } => draw_countdown(frame, room, &looks, *left),
        PhaseView::Results { outcome, next_in } => draw_results(frame, room, outcome, *next_in),
        _ => {}
    }
}

fn draw_scores(frame: &mut Frame, room: &InRoom, looks: &[SnakeLook], rect: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(dim())
        .title(" Players ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut seats: Vec<_> = room.view.seats.iter().collect();
    seats.sort_by_key(|s| std::cmp::Reverse((s.slot.is_some(), s.alive, s.length)));
    let followed = room.followed().map(|s| s.who.id);
    // Round trips go last, as numbers if names keep 8 columns, or else as
    // a dot in their color.
    let (ping_w, name_w) = match (inner.width as usize).saturating_sub(8) {
        w if w >= 8 + 6 => (6, w - 6),
        w if w >= 6 + 2 => (2, w - 2),
        w => (0, w),
    };
    let lines: Vec<Line> = seats
        .iter()
        .map(|seat| {
            let mut spans = Vec::new();
            match seat.slot.and_then(|slot| looks.get(slot as usize)) {
                Some(look) if seat.alive => spans.push(Span::styled(
                    format!(" {} ", seat.slot.unwrap_or(0) + 1),
                    Style::default()
                        .fg(Color::Black)
                        .bg(look.head)
                        .add_modifier(Modifier::BOLD),
                )),
                Some(_) => spans.push(Span::styled(" ✖ ", dim())),
                None => spans.push(Span::styled(" · ", dim())),
            }
            let name: String = seat.who.name.chars().take(name_w).collect();
            let style = if Some(seat.who.id) == followed {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if seat.alive {
                Style::default()
            } else {
                dim()
            };
            spans.push(Span::styled(format!(" {name:<name_w$}"), style));
            if seat.slot.is_some() {
                spans.push(Span::styled(format!("{:>4}", seat.length), style));
            } else if ping_w > 0 {
                spans.push(Span::raw("    "));
            }
            match (ping_w, seat.ping) {
                (0, _) => {}
                (2, Some(ping)) => spans.push(Span::styled(" ●", ping_style(ping))),
                (_, Some(ping)) => spans.push(Span::styled(
                    format!("{:>ping_w$}", ping_text(ping, true)),
                    ping_style(ping),
                )),
                (_, None) => {}
            }
            Line::from(spans)
        })
        .collect();
    let mut lines = lines;
    if room.view.watchers > 0 {
        lines.push(Line::default());
        lines.push(Line::styled(
            format!(" {} watching", room.view.watchers),
            dim(),
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn status_line(room: &InRoom, cropped: bool, need_w: u16, need_h: u16) -> Line<'static> {
    let warn = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    if room.confirm_leave {
        return Line::styled("Leave the match? y leave · n stay", warn);
    }
    if cropped {
        return Line::styled(
            format!("Terminal too small: showing part of the board (needs {need_w}×{need_h})"),
            Style::default().fg(Color::LightRed),
        );
    }
    if let Some(friend) = &room.watching {
        return Line::styled(format!("Watching {friend} · Esc leave"), dim());
    }
    let seat = room.my_seat();
    match &room.view.phase {
        PhaseView::Countdown { .. } => Line::styled("Get ready…", warn),
        PhaseView::Running { .. } if seat.is_some_and(|s| s.slot.is_none()) => {
            Line::styled("Watching · you'll play the next round · Esc leave", dim())
        }
        PhaseView::Running { .. } if !room.playing() => {
            Line::styled("You're out · watching the rest · Esc leave", dim())
        }
        PhaseView::Running { .. } => Line::styled("←↑↓→ move · Esc leave", dim()),
        _ => Line::default(),
    }
}

fn draw_countdown(frame: &mut Frame, room: &InRoom, looks: &[SnakeLook], left: Duration) {
    let inner = dialog(frame, " GET READY ", Color::Yellow, 26, 6);
    let mut lines = vec![
        Line::default(),
        Line::styled(
            secs(left).max(1).to_string(),
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(slot) = room.followed().and_then(|s| s.slot) {
        let look = looks.get(slot as usize);
        let who = match &room.watching {
            Some(friend) => format!("{friend} is "),
            None => "You're ".to_string(),
        };
        lines.push(Line::from(vec![
            Span::raw(who),
            Span::styled(
                format!(" {} ", slot + 1),
                Style::default()
                    .fg(Color::Black)
                    .bg(look.map_or(Color::White, |l| l.head))
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    } else if let Some(friend) = &room.watching {
        lines.push(Line::styled(format!("Watching {friend}"), dim()));
    } else {
        lines.push(Line::styled("You'll play the next round", dim()));
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn draw_results(frame: &mut Frame, room: &InRoom, outcome: &Outcome, next_in: Option<Duration>) {
    let view = &room.view;
    let height = 9 + view.placements.len() as u16;
    let (headline, color) = match outcome {
        Outcome::Won { winner, .. } if winner.id == room.me => {
            ("★ You win! ★".to_string(), Color::LightGreen)
        }
        Outcome::Won { winner, .. } => (format!("{} wins!", named(winner)), Color::LightCyan),
        Outcome::Draw { .. } => ("Draw!".to_string(), Color::LightYellow),
    };
    let inner = dialog(frame, " RESULTS ", color, 48, height);
    let time_up = matches!(
        outcome,
        Outcome::Won { time_up: true, .. } | Outcome::Draw { time_up: true }
    );
    let mut lines = vec![
        Line::default(),
        Line::styled(
            headline,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        if time_up {
            Line::styled("Time's up", dim())
        } else {
            Line::default()
        },
    ];
    for p in &view.placements {
        let style = if Some(p.who.id) == room.followed().map(|s| s.who.id) {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let name: String = named(&p.who).chars().take(18).collect();
        let mut spans = vec![Span::styled(
            format!("{:>2}. {name:<18} {:>4}", p.place, p.length),
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
