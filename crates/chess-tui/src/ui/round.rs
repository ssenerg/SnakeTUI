//! A game in a room: the board, and beside it (or around it, on a narrow
//! terminal) the players' clocks, what each has taken, the moves so far,
//! and whose move it is.

use std::time::Duration;

use chess_core::moves::Move;
use chess_core::piece::{self, Piece};
use chess_core::position::Position;
use chess_core::san;
use chess_core::square::Square;
use chess_core::view::GameView;
use game_api::{PhaseView, SeatView};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use shell_tui::Ctx;
use shell_tui::online::InRoom;
use shell_tui::ui::{centered_rect, dim, secs};

use super::board::{self, Marks, SQUARES, SquareSize, board_size};
use crate::app::{ChessUi, MenuItem, PROMOTIONS, bottom, chess_view, my_color};
use crate::entry;
use crate::prefs::MoveInput;

/// The panel beside the board: this wide, or down to the least.
const PANEL_W: u16 = 36;
const PANEL_MIN_W: u16 = 24;
/// Rows the panel needs: the players and what they took, a move, whose
/// move it is, and what's being typed.
const PANEL_MIN_H: u16 = 9;
/// Rows a stacked layout adds to the board: a player above it and one
/// below, then the status and what's being typed.
const STACKED_ROWS: u16 = 4;

/// Where everything goes on screen.
struct Layout {
    square: SquareSize,
    board: (u16, u16),
    /// The panel beside the board; `None` when the players go above and
    /// below it.
    panel: Option<Rect>,
}

fn layout(area: Rect, coords: bool) -> Option<Layout> {
    for square in SQUARES {
        let (bw, bh) = board_size(square, coords);
        let spare = area.width.saturating_sub(bw + 1);
        if spare >= PANEL_MIN_W && area.height >= bh.max(PANEL_MIN_H) {
            let pw = spare.min(PANEL_W);
            let h = bh.max(PANEL_MIN_H);
            let x = area.x + (area.width - (bw + 1 + pw)) / 2;
            let y = area.y + (area.height - h) / 2;
            return Some(Layout {
                square,
                board: (x, y + (h - bh) / 2),
                panel: Some(Rect::new(x + bw + 1, y, pw, h)),
            });
        }
        if area.width >= bw && area.height >= bh + STACKED_ROWS {
            let x = area.x + (area.width - bw) / 2;
            let y = area.y + (area.height - bh - STACKED_ROWS) / 2;
            return Some(Layout {
                square,
                board: (x, y + 1),
                panel: None,
            });
        }
    }
    None
}

/// The smallest terminal that shows a board.
pub fn smallest(coords: bool) -> (u16, u16) {
    let (bw, bh) = board_size(SQUARES[SQUARES.len() - 1], coords);
    (bw, bh + STACKED_ROWS)
}

/// A clock as shown: `m:ss`, or `h:mm:ss` from an hour.
pub fn clock(left: Duration) -> String {
    let s = secs(left);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// What a game is drawn from.
struct Scene<'a> {
    ui: &'a ChessUi,
    room: &'a InRoom,
    game: &'a GameView,
    position: Position,
    sans: Vec<String>,
    phase: &'a PhaseView,
    /// The side this player plays, while their game goes on.
    me: Option<piece::Color>,
    bottom: piece::Color,
    /// Something the app has to tell the player.
    notice: Option<&'a str>,
}

impl Scene<'_> {
    fn seat(&self, color: piece::Color) -> Option<&SeatView> {
        let slot = self.game.players.get(color);
        self.room
            .shell()
            .seats
            .iter()
            .find(|s| s.slot.map(u64::from) == Some(slot))
    }

    fn name(&self, color: piece::Color) -> String {
        let Some(seat) = self.seat(color) else {
            return color.name().to_string();
        };
        let mut name = seat.who.name.clone();
        if seat.who.id == self.room.me {
            name.push_str(" (you)");
        } else if seat.left {
            name.push_str(" (left)");
        } else if seat.away.is_some() {
            name.push_str(" (away)");
        }
        name
    }

    /// The player's name and clock, across `width`.
    fn player(&self, color: piece::Color, width: u16) -> Line<'static> {
        let left = self.game.clocks[color.index()];
        let time = clock(left);
        let running = self.game.running == Some(color) && self.game.outcome.is_none();
        let clock_style = match (running, secs(left) <= 10) {
            (true, true) => Style::default().fg(Color::White).bg(Color::Red),
            (true, false) => Style::default().fg(Color::Black).bg(Color::White),
            (false, _) => Style::default().fg(Color::Gray),
        }
        .add_modifier(Modifier::BOLD);
        let turn = self.position.side_to_move() == color && self.game.outcome.is_none();
        let name_style = if turn {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let room = (width as usize).saturating_sub(time.chars().count() + 3);
        let name: String = self.name(color).chars().take(room).collect();
        let gap = (width as usize).saturating_sub(name.chars().count() + time.chars().count() + 2);
        Line::from(vec![
            Span::styled(name, name_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(format!(" {time} "), clock_style),
        ])
    }

    /// What `color` has taken, and how far ahead in material they are.
    fn taken(&self, color: piece::Color) -> Line<'static> {
        const ORDER: [(Piece, u8, i32); 5] = [
            (Piece::Pawn, 8, 1),
            (Piece::Knight, 2, 3),
            (Piece::Bishop, 2, 3),
            (Piece::Rook, 2, 5),
            (Piece::Queen, 1, 9),
        ];
        let mut text = String::new();
        let mut lead = 0;
        for (piece, start, worth) in ORDER {
            let left = |c: piece::Color| {
                (0..64)
                    .filter_map(Square::from_index)
                    .filter(|&sq| self.position.piece_at(sq) == Some((c, piece)))
                    .count() as i32
            };
            let theirs = left(!color);
            let taken = (i32::from(start) - theirs).max(0);
            for _ in 0..taken {
                text.push_str(board::symbol(self.ui.prefs.pieces, !color, piece));
            }
            lead += (left(color) - theirs) * worth;
        }
        if lead > 0 {
            text.push_str(&format!(" +{lead}"));
        }
        Line::styled(text, dim())
    }

    /// The moves so far, a row a move each side, the last `rows` of them.
    fn moves(&self, rows: usize) -> Vec<Line<'static>> {
        let last = self.sans.len().checked_sub(1);
        let mut lines: Vec<Line> = self
            .sans
            .chunks(2)
            .enumerate()
            .map(|(n, pair)| {
                let style = |i: usize| {
                    if Some(n * 2 + i) == last {
                        Style::default()
                            .fg(Color::LightYellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    }
                };
                let mut spans = vec![
                    Span::styled(format!("{:>3}. ", n + 1), dim()),
                    Span::styled(format!("{:<8}", pair[0]), style(0)),
                ];
                if let Some(black) = pair.get(1) {
                    spans.push(Span::styled(black.clone(), style(1)));
                }
                Line::from(spans)
            })
            .collect();
        let skip = lines.len().saturating_sub(rows);
        lines.drain(..skip);
        lines
    }

    /// Whose move it is, or how the game ended, or what's being asked.
    fn status(&self) -> Line<'static> {
        let game = self.game;
        let bold = Style::default().add_modifier(Modifier::BOLD);
        if let Some(outcome) = game.outcome {
            return Line::styled(outcome.to_string(), bold.fg(Color::LightYellow));
        }
        if let PhaseView::Countdown { left } = self.phase {
            // Esc before the game starts asks about leaving the room.
            if self.room.confirm_leave {
                let ask = "Leave the room? y leave · n stay";
                return Line::styled(ask, bold.fg(Color::Yellow));
            }
            return Line::styled(format!("Starting in {} s", secs(*left)), bold);
        }
        // A player whose connection went, while their seat waits.
        let away = piece::Color::BOTH.iter().find_map(|&side| {
            let seat = self.seat(side).filter(|s| s.who.id != self.room.me)?;
            Some((seat, seat.away?))
        });
        if let Some((seat, left)) = away {
            let text = format!("{} is away · back within {}", seat.who.name, clock(left));
            return Line::styled(text, Style::default().fg(Color::LightYellow));
        }
        let to_move = self.position.side_to_move();
        let check = if self.position.in_check() {
            " · check"
        } else {
            ""
        };
        if let Some(by) = game.draw_offer {
            let text = if Some(by) == self.me {
                "You offered a draw".to_string()
            } else if self.me.is_some() {
                format!("{} offers a draw · Esc to answer", self.name(by))
            } else {
                format!("{} offers a draw", by.name())
            };
            return Line::styled(text, Style::default().fg(Color::LightYellow));
        }
        let whose = if Some(to_move) == self.me {
            "Your move".to_string()
        } else {
            format!("{} to move", capitalized(to_move.name()))
        };
        let text = match game.first_move_deadline {
            Some(by) => format!("{whose} within {} s", secs(by.saturating_sub(game.at))),
            None => whose,
        };
        let style = if Some(to_move) == self.me {
            bold.fg(Color::LightGreen)
        } else {
            Style::default()
        };
        Line::styled(format!("{text}{check}"), style)
    }

    /// What's being typed and what it could be, or a problem, or a hint.
    fn input(&self) -> Line<'static> {
        let desk = &self.ui.desk;
        if self.me.is_none() {
            let hint = match &self.room.watching {
                Some(friend) => format!("Watching {friend} · f flip · Esc leave"),
                None => "f flip the board".to_string(),
            };
            return Line::styled(hint, dim());
        }
        if let Some(notice) = desk.notice.or(self.notice) {
            return Line::styled(notice.to_string(), Style::default().fg(Color::LightYellow));
        }
        if desk.typed.is_empty() {
            let hint = match self.ui.prefs.input {
                MoveInput::Typed => "Type or arrows · Esc menu",
                MoveInput::Cursor => "Arrows · Enter · Esc menu",
            };
            return Line::styled(hint, dim());
        }
        let mut spans = vec![Span::styled(
            format!("› {}", desk.typed),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )];
        if self.my_turn() {
            let found = entry::candidates(&self.position, &desk.typed);
            let names: Vec<String> = found
                .iter()
                .take(4)
                .filter_map(|&m| san::format(&self.position, m))
                .collect();
            let more = if found.len() > 4 { " …" } else { "" };
            let text = if found.is_empty() {
                "  no such move".to_string()
            } else {
                format!("  {}{more}", names.join(" "))
            };
            spans.push(Span::styled(text, dim()));
        }
        Line::from(spans)
    }

    fn my_turn(&self) -> bool {
        self.me == Some(self.position.side_to_move())
    }

    /// The last move, a check, and how this player is entering theirs.
    fn marks(&self) -> Marks {
        let desk = &self.ui.desk;
        let mut marks = Marks {
            last: self.game.moves.last().copied().and_then(Move::unpack),
            check: self
                .position
                .in_check()
                .then(|| self.position.king(self.position.side_to_move())),
            ..Marks::default()
        };
        let Some(me) = self.me else {
            return marks;
        };
        marks.cursor = desk.cursor;
        let mine = |sq| self.position.piece_at(sq).is_some_and(|(c, _)| c == me);
        if let Some(from) = desk.selected.filter(|&sq| mine(sq)) {
            marks.from.push(from);
            if self.my_turn() {
                marks.to = self
                    .position
                    .legal_moves()
                    .iter()
                    .filter(|m| m.from == from)
                    .map(|m| m.to)
                    .collect();
            }
        } else if self.my_turn() && !desk.typed.is_empty() {
            let found = entry::candidates(&self.position, &desk.typed);
            for m in found {
                if !marks.from.contains(&m.from) {
                    marks.from.push(m.from);
                }
                if !marks.to.contains(&m.to) {
                    marks.to.push(m.to);
                }
            }
        }
        marks
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

pub(crate) fn draw_round(frame: &mut Frame, ui: &ChessUi, room: &InRoom, ctx: &Ctx) {
    let Some(view) = chess_view(room) else {
        return;
    };
    let Some(game) = &view.game else {
        return;
    };
    let Some((position, sans)) = game.replay() else {
        return;
    };
    let phase = &view.shell.phase;
    let playing = matches!(phase, PhaseView::Running { .. }) && game.outcome.is_none();
    let scene = Scene {
        ui,
        room,
        game,
        position,
        sans,
        phase,
        me: my_color(room, game).filter(|_| playing),
        bottom: bottom(room, game, ui.desk.flipped),
        notice: ctx.notice,
    };
    let area = frame.area();
    let Some(layout) = layout(area, ui.prefs.coordinates) else {
        let (w, h) = smallest(ui.prefs.coordinates);
        let text = format!("Make the terminal at least {w}×{h} to see the board.");
        frame.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Center)
                .wrap(ratatui::widgets::Wrap { trim: true }),
            centered_rect(area, area.width, 3),
        );
        return;
    };
    board::draw(
        frame.buffer_mut(),
        layout.board,
        &scene.position,
        layout.square,
        scene.bottom,
        &ui.prefs,
        &scene.marks(),
    );
    let (top, under) = (!scene.bottom, scene.bottom);
    let (bw, bh) = board_size(layout.square, ui.prefs.coordinates);
    match layout.panel {
        Some(panel) => {
            let w = panel.width;
            let moves = panel.height.saturating_sub(7) as usize;
            let mut lines = vec![scene.player(top, w), scene.taken(top)];
            let shown = scene.moves(moves);
            let pad = moves - shown.len();
            lines.extend(shown);
            lines.extend((0..pad).map(|_| Line::default()));
            lines.extend([
                scene.taken(under),
                scene.player(under, w),
                Line::default(),
                scene.status(),
                scene.input(),
            ]);
            frame.render_widget(Paragraph::new(lines), panel);
        }
        None => {
            let (x, y) = layout.board;
            let row = |dy: u16| Rect::new(x, y + dy, bw, 1);
            let line = |frame: &mut Frame, at: Rect, line: Line| {
                frame.render_widget(Paragraph::new(line), at);
            };
            line(frame, Rect { y: y - 1, ..row(0) }, scene.player(top, bw));
            line(frame, row(bh), scene.player(under, bw));
            line(frame, row(bh + 1), scene.status());
            line(frame, row(bh + 2), scene.input());
        }
    }
    let board_area = board::area_of(layout.board, layout.square, ui.prefs.coordinates);
    if let Some(me) = scene.me {
        draw_asking(frame, ui, game, me, board_area);
    }
}

/// What this player opened, or is being asked, over the board.
fn draw_asking(frame: &mut Frame, ui: &ChessUi, game: &GameView, me: piece::Color, over: Rect) {
    let desk = &ui.desk;
    let chosen = Style::default()
        .fg(Color::Black)
        .bg(Color::LightYellow)
        .add_modifier(Modifier::BOLD);
    let mut box_over = |title: &str, lines: Vec<Line<'static>>, width: u16| {
        let height = lines.len() as u16 + 2;
        let rect = centered_rect(over, width, height);
        frame.render_widget(ratatui::widgets::Clear, rect);
        let block = ratatui::widgets::Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(title.to_string())
            .title_alignment(Alignment::Center)
            .border_style(Style::default().fg(Color::LightYellow));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
    };
    if let Some((_, _, at)) = desk.promoting {
        let mut spans = Vec::new();
        for (i, piece) in PROMOTIONS.into_iter().enumerate() {
            let name = format!(" {} ", board::symbol(ui.prefs.pieces, me, piece));
            spans.push(if i == at {
                Span::styled(name, chosen)
            } else {
                Span::raw(name)
            });
            spans.push(Span::raw(" "));
        }
        let hint = Line::styled("←→ choose · Enter · q r b n", dim());
        box_over(" Promote to ", vec![Line::from(spans), hint], 30);
    } else if desk.confirm_resign {
        let lines = vec![
            Line::from("Resign this game?"),
            Line::styled("y resign · any other key keeps playing", dim()),
        ];
        box_over(" Resign ", lines, 42);
    } else if let Some(row) = desk.menu {
        let items = MenuItem::offered(game, me);
        let row = row.min(items.len() - 1);
        let lines = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if i == row {
                    Line::styled(format!(" {} ", item.label()), chosen)
                } else {
                    Line::from(item.label())
                }
            })
            .collect();
        box_over(" Game ", lines, 24);
    }
}
