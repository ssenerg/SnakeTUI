//! Chess's screens: the board and its settings, a room's challenge, and
//! games.

mod board;
mod round;

use chess_core::piece;
use chess_core::position::Position;
use chess_core::round::ChessSettings;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use shell_tui::Ctx;
use shell_tui::ui::{centered_rect, dim};

use crate::app::{ChessScreen, ChessUi, chess_view, speed};
use crate::prefs::{BoardLook, MoveInput, PieceStyle, Prefs};

pub use board::{SQUARES, board_size};
pub(crate) use round::draw_round;

pub const LOGO: [&str; 3] = ["┌─┐┬ ┬┌─┐┌─┐┌─┐", "│  ├─┤├┤ └─┐└─┐", "└─┘┴ ┴└─┘└─┘└─┘"];

/// One row of the settings screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Board,
    Pieces,
    Coordinates,
    AutoQueen,
    Input,
    ConfirmResign,
}

pub const SETTINGS: [Setting; 6] = [
    Setting::Board,
    Setting::Pieces,
    Setting::Coordinates,
    Setting::AutoQueen,
    Setting::Input,
    Setting::ConfirmResign,
];

/// The value after `now` among `all`, or before it when `back`.
fn cycle<T: Copy + PartialEq>(all: &[T], now: T, back: bool) -> T {
    let i = all.iter().position(|&v| v == now).unwrap_or(0);
    let n = all.len();
    all[if back { (i + n - 1) % n } else { (i + 1) % n }]
}

impl Setting {
    pub fn label(self) -> &'static str {
        match self {
            Setting::Board => "Board",
            Setting::Pieces => "Pieces",
            Setting::Coordinates => "Coordinates",
            Setting::AutoQueen => "Promote to a queen",
            Setting::Input => "Letter keys",
            Setting::ConfirmResign => "Ask before resigning",
        }
    }

    pub fn value(self, prefs: &Prefs) -> &'static str {
        let yes_no = |on: bool| if on { "yes" } else { "no" };
        match self {
            Setting::Board => prefs.board.name(),
            Setting::Pieces => prefs.pieces.name(),
            Setting::Coordinates => yes_no(prefs.coordinates),
            Setting::AutoQueen => {
                if prefs.auto_queen {
                    "always"
                } else {
                    "ask"
                }
            }
            Setting::Input => match prefs.input {
                MoveInput::Typed => "type moves",
                MoveInput::Cursor => "move the cursor",
            },
            Setting::ConfirmResign => yes_no(prefs.confirm_resign),
        }
    }

    pub fn change(self, prefs: &mut Prefs, back: bool) {
        match self {
            Setting::Board => prefs.board = cycle(&BoardLook::ALL, prefs.board, back),
            Setting::Pieces => prefs.pieces = cycle(&PieceStyle::ALL, prefs.pieces, back),
            Setting::Coordinates => prefs.coordinates = !prefs.coordinates,
            Setting::AutoQueen => prefs.auto_queen = !prefs.auto_queen,
            Setting::Input => prefs.input = cycle(&MoveInput::ALL, prefs.input, back),
            Setting::ConfirmResign => prefs.confirm_resign = !prefs.confirm_resign,
        }
    }
}

pub(crate) fn draw(frame: &mut Frame, ui: &ChessUi, ctx: &Ctx) {
    match ui.screen {
        ChessScreen::Settings => draw_settings(frame, ui),
        ChessScreen::Challenge { selected } => draw_challenge(frame, ui, ctx, selected),
    }
}

/// A rounded box with `title`, `rows` tall inside, at the top of `area`;
/// returns the space inside.
fn framed(frame: &mut Frame, area: Rect, title: &str, width: u16, rows: u16) -> Rect {
    let rect = Rect {
        y: area.y,
        ..centered_rect(area, width, rows + 2)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title.to_string())
        .title_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .title_alignment(Alignment::Center);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// A row of a settings list: its label, and its value with arrows when it
/// can be changed here.
fn row(label: &str, value: &str, active: bool, changeable: bool) -> Line<'static> {
    let (marker, label_style) = if active {
        (
            "❯ ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("  ", Style::default())
    };
    let value = if active && changeable {
        Span::styled(
            format!("◂ {value} ▸"),
            Style::default().fg(Color::LightYellow),
        )
    } else {
        Span::styled(format!("  {value}  "), dim())
    };
    Line::from(vec![
        Span::styled(format!("{marker}{label:<22}"), label_style),
        value,
    ])
}

fn footer(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let height = (lines.len() as u16).min(area.height);
    let rect = Rect {
        y: area.bottom() - height,
        height,
        ..area
    };
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        rect,
    );
}

fn draw_settings(frame: &mut Frame, ui: &ChessUi) {
    let area = frame.area();
    let width = 48;
    let rows = SETTINGS.len() as u16;
    let inner = framed(frame, area, " Board and pieces ", width, rows);
    let lines: Vec<Line> = SETTINGS
        .iter()
        .enumerate()
        .map(|(i, s)| row(s.label(), s.value(&ui.prefs), i == ui.selected, true))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    // A board as these settings draw it, if it fits.
    let square = SQUARES[1];
    let (bw, bh) = board_size(square, ui.prefs.coordinates);
    let top = inner.bottom() + 2;
    if area.height >= top + bh + 2 && area.width >= bw {
        let x = area.x + (area.width - bw) / 2;
        board::draw(
            frame.buffer_mut(),
            (x, top),
            &Position::start(),
            square,
            piece::Color::White,
            &ui.prefs,
            &board::Marks::default(),
        );
    }
    footer(
        frame,
        area,
        vec![Line::styled("↑↓ select · ←→ change · Esc back", dim())],
    );
}

/// Minutes as a challenge shows them: ¼, ½, ¾, 1½, or whole.
fn minutes(settings: &ChessSettings) -> String {
    let s = settings.control.base.as_secs();
    match s {
        15 => "¼".to_string(),
        30 => "½".to_string(),
        45 => "¾".to_string(),
        90 => "1½".to_string(),
        s if s % 60 == 0 => (s / 60).to_string(),
        s => format!("{:.2}", s as f64 / 60.0),
    }
}

fn draw_challenge(frame: &mut Frame, ui: &ChessUi, ctx: &Ctx, selected: usize) {
    let Some((room, view)) = ctx.room.and_then(|r| Some((r, chess_view(r)?))) else {
        return;
    };
    let area = frame.area();
    let settings = view.settings;
    let hosting = room.hosting();
    let inner = framed(frame, area, " Challenge ", 48, 5);
    let increment = format!("{} s", settings.control.increment.as_secs());
    let side = settings.side.name();
    let host_side = if hosting {
        "You play"
    } else {
        "The host plays"
    };
    let mut lines = vec![
        row("Minutes each", &minutes(&settings), selected == 0, hosting),
        row("Added each move", &increment, selected == 1, hosting),
        row(host_side, side, selected == 2, hosting),
        Line::default(),
        Line::styled(
            format!(
                "{} · {} · unrated",
                settings.control,
                speed(settings.control)
            ),
            dim(),
        )
        .alignment(Alignment::Center),
    ];
    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
    let _ = ui;
    let status = match (ctx.notice, hosting) {
        (Some(notice), _) => Line::styled(
            notice.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        (None, true) => Line::styled("A change un-readies everyone", dim()),
        (None, false) => {
            let host = view
                .shell
                .host()
                .map_or("the host", |h| h.who.name.as_str());
            Line::styled(format!("Only {host} can change these."), dim())
        }
    };
    let hint = if hosting {
        "↑↓ select · ←→ change · Esc back"
    } else {
        "Esc back"
    };
    footer(frame, area, vec![status, Line::styled(hint, dim())]);
}
