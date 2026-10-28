//! A room's round: the board with its scoreboard, from the countdown to
//! under the results.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use shell_tui::GameUi;
use shell_tui::online::InRoom;
use shell_tui::ui::{dialog, dim, ping_style, ping_text, secs};
use snake_core::config::ColorSlot;
use snake_core::geom::Point;
use snake_core::round::{PhaseView, RoomView};
use snake_core::view::MatchView;

use super::board::{BoardWidget, SnakeLook};
use crate::app::{SnakeUi, racer, snake_view};
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

pub(crate) fn draw_round(frame: &mut Frame, app: &SnakeUi, room: &InRoom) {
    if let Some(view) = snake_view(room)
        && let Some(board) = &view.game.board
    {
        draw_match(frame, app, room, view, board);
    }
}

/// How each snake on `board` is drawn for this player: their own in their
/// theme's colors, the others in colors that stand apart from it.
fn looks(app: &SnakeUi, room: &InRoom, board: &MatchView) -> Vec<SnakeLook> {
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

fn draw_match(frame: &mut Frame, app: &SnakeUi, room: &InRoom, view: &RoomView, board: &MatchView) {
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

    let clock = match &view.shell.phase {
        PhaseView::Running { left: Some(left) } => {
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
        draw_scores(frame, room, view, &looks, score_rect);
    }
    let (need_w, need_h) = room_size(board.width, board.height);
    let playing = app.in_play(room);
    frame.render_widget(
        Paragraph::new(status_line(room, playing, cropped, need_w, need_h))
            .alignment(Alignment::Center),
        status,
    );
    if let PhaseView::Countdown { left } = &view.shell.phase {
        draw_countdown(frame, room, &looks, *left);
    }
}

fn draw_scores(frame: &mut Frame, room: &InRoom, view: &RoomView, looks: &[SnakeLook], rect: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(dim())
        .title(" Players ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // Each seat with whether its snake is alive, and its length.
    let mut seats: Vec<_> = view
        .shell
        .seats
        .iter()
        .map(|s| {
            let racer = racer(view, s);
            let alive = racer.is_some_and(|r| r.alive);
            (s, alive, racer.map_or(0, |r| r.length))
        })
        .collect();
    seats.sort_by_key(|&(s, alive, length)| std::cmp::Reverse((s.slot.is_some(), alive, length)));
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
        .map(|&(seat, alive, length)| {
            let mut spans = Vec::new();
            match seat.slot.and_then(|slot| looks.get(slot as usize)) {
                Some(look) if alive => spans.push(Span::styled(
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
            } else if alive {
                Style::default()
            } else {
                dim()
            };
            spans.push(Span::styled(format!(" {name:<name_w$}"), style));
            if seat.slot.is_some() {
                spans.push(Span::styled(format!("{length:>4}"), style));
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
    if view.shell.watchers > 0 {
        lines.push(Line::default());
        lines.push(Line::styled(
            format!(" {} watching", view.shell.watchers),
            dim(),
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn status_line(
    room: &InRoom,
    playing: bool,
    cropped: bool,
    need_w: u16,
    need_h: u16,
) -> Line<'static> {
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
    match &room.shell().phase {
        PhaseView::Countdown { .. } => Line::styled("Get ready…", warn),
        PhaseView::Running { .. } if seat.is_some_and(|s| s.slot.is_none()) => {
            Line::styled("Watching · you'll play the next round · Esc leave", dim())
        }
        PhaseView::Running { .. } if !playing => {
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
