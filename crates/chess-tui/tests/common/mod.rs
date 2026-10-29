//! Chess rooms set up by hand for the tests: a private room with two
//! players, played to whatever point a test needs.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use chess_core::control::TimeControl;
use chess_core::moves::Move;
use chess_core::round::{Chess, ChessInput, ChessSettings, Side};
use chess_tui::ChessUi;
use game_api::{Entrant, PhaseView, Room, RoomKind, RoomSettings, RoomView};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Size;
use shell_tui::App;

/// Ann hosts and plays white; Ben plays black.
pub const ANN: u64 = 1;
pub const BEN: u64 = 2;

pub fn entrant(id: u64) -> Entrant {
    let name = match id {
        ANN => "ann-with-a-long-name",
        BEN => "ben",
        _ => "someone",
    };
    Entrant {
        id,
        name: name.to_string(),
    }
}

/// A private room of Ann's, where she plays white at `control`, with Ben
/// in it too.
pub fn room(control: TimeControl) -> Room<Chess> {
    let settings = ChessSettings {
        control,
        side: Side::White,
        ..ChessSettings::default()
    };
    let mut room = Room::new(RoomSettings::new(settings, RoomKind::Private).unwrap(), 1);
    room.join(entrant(ANN)).unwrap();
    room.join(entrant(BEN)).unwrap();
    room
}

/// Ann's private room at 5+3, where she plays white against the computer
/// at `level`.
pub fn against_computer(level: u8) -> Room<Chess> {
    let settings = ChessSettings {
        control: TimeControl::minutes(5, 3),
        side: Side::White,
        level,
    };
    let mut room = Room::new(RoomSettings::new(settings, RoomKind::Private).unwrap(), 1);
    room.join(entrant(ANN)).unwrap();
    assert!(room.add_bot(ANN));
    room
}

/// Ann's room with the game under way, after these moves in UCI, a second
/// apart, white first.
pub fn playing(moves: &[&str]) -> Room<Chess> {
    let mut room = room(TimeControl::minutes(5, 3));
    room.toggle_ready(ANN);
    room.toggle_ready(BEN);
    room.advance(Duration::from_secs(3));
    assert!(matches!(room.view().shell.phase, PhaseView::Running { .. }));
    for (i, uci) in moves.iter().enumerate() {
        room.advance(Duration::from_secs(1));
        let by = if i % 2 == 0 { ANN } else { BEN };
        let mv = Move::parse_uci(uci).unwrap();
        room.play(by, ChessInput::Move(mv), None);
    }
    room
}

/// The app with chess as its game, in `view`'s room as `me`.
pub fn app_in(me: u64, view: RoomView<Chess>) -> App {
    let mut app = App::online(
        vec![Box::new(ChessUi::default())],
        None,
        "~ann".into(),
        true,
    );
    app.resize(Size::new(80, 24));
    assert!(app.enter_room(me, Arc::new(view)));
    app
}

pub fn chess(app: &mut App) -> &mut ChessUi {
    app.game_ui_mut().expect("chess")
}

/// The app's screen, drawn on a `w`×`h` terminal, as text.
pub fn shown(app: &App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|frame| shell_tui::ui::draw(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sends `keys` to the app as if typed, `\r` as Enter, returning what it
/// asked for.
pub fn typed(app: &mut App, keys: &str) -> Vec<shell_tui::Effect> {
    keys.chars()
        .flat_map(|c| {
            app.handle(match c {
                '\r' => shell_tui::Key::Enter,
                c => shell_tui::Key::Char(c),
            })
        })
        .collect()
}
