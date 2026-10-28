//! Over SSH a panic while drawing drops the player's connection, so every
//! chess screen must draw at every terminal size.

mod common;

use std::time::{Duration, Instant};

use chess_core::control::{PRESETS, TimeControl};
use chess_core::moves::Move;
use chess_core::round::Chess;
use chess_core::round::ChessInput;
use chess_tui::{BoardLook, ChessScreen, MoveInput, PieceStyle};
use common::*;
use game_api::{GameId, PhaseView, RatingKey, RoomView};
use shell_tui::online::{HomeItem, Profile, ShownRating};
use shell_tui::{App, Key, Screen};

/// Every phase of a room worth drawing, with its moves and offers.
fn views() -> Vec<RoomView<Chess>> {
    let mut views = Vec::new();
    let mut lobby = room(TimeControl::minutes(10, 0));
    let mut alone = lobby.view();
    alone.shell.seats.truncate(1);
    views.push(alone);
    let mut coded = lobby.view();
    coded.shell.code = Some("K7M2QX".into());
    views.push(coded);
    lobby.toggle_ready(ANN);
    lobby.toggle_ready(BEN);
    views.push(lobby.view());
    views.push(playing(&[]).view());
    let opening = [
        "e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5c6", "d7c6",
    ];
    views.push(playing(&opening).view());
    // Check, a draw offer, and a long game.
    views.push(playing(&["e2e4", "f7f6", "d2d4", "g7g5", "d1h5"]).view());
    let mut offered = playing(&opening);
    offered.play(BEN, ChessInput::OfferDraw, None);
    views.push(offered.view());
    let shuffle = ["g1f3", "g8f6", "f3g1", "f6g8"];
    let long: Vec<&str> = shuffle.iter().cycle().take(60).copied().collect();
    views.push(playing(&long).view());
    // Clocks: one about to fall, one over an hour.
    let mut late = playing(&["e2e4", "e7e5"]).view();
    late.game.as_mut().unwrap().clocks[0] = Duration::from_millis(4300);
    views.push(late);
    let mut slow = playing(&["e2e4"]).view();
    slow.game.as_mut().unwrap().clocks = [Duration::from_secs(3 * 3600); 2];
    views.push(slow);
    // How games end: mate, a draw, an abort.
    let mut mated = playing(&["f2f3", "e7e5", "g2g4"]);
    mated.play(
        BEN,
        ChessInput::Move(Move::parse_uci("d8h4").unwrap()),
        None,
    );
    views.push(mated.view());
    let mut drawn = playing(&opening);
    drawn.play(ANN, ChessInput::OfferDraw, None);
    drawn.play(BEN, ChessInput::AcceptDraw, None);
    views.push(drawn.view());
    let mut aborted = playing(&[]);
    aborted.play(ANN, ChessInput::Abort, None);
    views.push(aborted.view());
    let mut left = playing(&opening);
    left.leave(BEN);
    views.push(left.view());
    assert!(
        views
            .iter()
            .any(|v| matches!(v.shell.phase, PhaseView::Results { .. }))
    );
    views
}

/// The app as `me` in `view`, with `prefs` changed by `tweak`.
fn app(me: u64, view: &RoomView<Chess>, tweak: impl Fn(&mut App)) -> App {
    let mut app = app_in(me, view.clone());
    tweak(&mut app);
    app
}

/// Out of the room, on the home menu.
fn home(app: &mut App) {
    app.online.room = None;
    app.go_home(HomeItem::CreateRoom, None);
}

fn screens() -> Vec<App> {
    let mut apps = Vec::new();
    let views = views();
    let looks: [fn(&mut App); 4] = [
        |_| {},
        |app| {
            let prefs = &mut chess(app).prefs;
            prefs.pieces = PieceStyle::Letters;
            prefs.coordinates = false;
            prefs.board = BoardLook::Blue;
        },
        |app| {
            chess(app).prefs.input = MoveInput::Cursor;
            app.handle(Key::Up);
            app.handle(Key::Enter);
        },
        |app| {
            typed(app, "Nf");
        },
    ];
    for view in &views {
        for me in [ANN, BEN, 99] {
            apps.push(app(me, view, |_| {}));
        }
        for look in looks {
            apps.push(app(ANN, view, look));
        }
    }
    // What's opened over the board: the menu, a promotion, resigning.
    let opening = playing(&["e2e4", "e7e5"]).view();
    apps.push(app(ANN, &opening, |app| {
        app.handle(Key::Esc);
    }));
    apps.push(app(ANN, &opening, |app| {
        app.handle(Key::Esc);
        app.handle(Key::Down);
        app.handle(Key::Enter);
    }));
    let mut promoting = playing(&[]).view();
    promoting.game.as_mut().unwrap().start = Some("8/4P3/8/8/8/8/k7/4K3 w - - 0 1".into());
    apps.push(app(ANN, &promoting, |app| {
        typed(app, "e8\r");
    }));
    apps.push(app(ANN, &opening, |app| {
        typed(app, "Qh9\r");
    }));
    // The other side's connection went, and their seat waits; and back
    // in a game after one's own did.
    let mut away = opening.clone();
    away.shell.seats[1].away = Some(Duration::from_secs(75));
    apps.push(app(ANN, &away, |_| {}));
    apps.push(app(ANN, &opening, |app| {
        app.notify("Welcome back: your game went on.".into());
    }));
    // Esc in the countdown asks about leaving.
    apps.push(app(ANN, &views[2], |app| {
        app.handle(Key::Esc);
        assert!(app.online.room.as_ref().unwrap().confirm_leave);
    }));
    // Watching, flipped.
    apps.push(app(99, &opening, |app| {
        app.online.room.as_mut().unwrap().watching = Some("ann".into());
        typed(app, "f");
    }));
    // Chess's own screens: the settings and a room's challenge.
    for selected in [0, 5] {
        let mut app = app(ANN, &views[1], home);
        app.screen = Screen::Game;
        let ui = chess(&mut app);
        ui.screen = ChessScreen::Settings;
        ui.selected = selected;
        apps.push(app);
    }
    for (me, selected) in [(ANN, 0), (ANN, 2), (BEN, 1)] {
        let mut app = app(me, &views[1], |_| {});
        app.screen = Screen::Game;
        chess(&mut app).screen = ChessScreen::Challenge { selected };
        apps.push(app);
    }
    let mut noted = app(BEN, &views[1], |_| {});
    noted.screen = Screen::Game;
    chess(&mut noted).screen = ChessScreen::Challenge { selected: 0 };
    noted.notify("ann-with-a-long-name made it 3+2 blitz".into());
    apps.push(noted);
    // The home menu; quick pairing's presets, with every one chosen, a
    // signed-in player's ratings and a notice; and waiting in a pool.
    apps.push(app(ANN, &views[1], home));
    for selected in 0..PRESETS.len() {
        let mut app = app(ANN, &views[1], home);
        app.go_home(HomeItem::QuickPlay, None);
        app.handle(Key::Enter);
        chess(&mut app).screen = ChessScreen::Pairing { selected };
        apps.push(app);
    }
    let rated = |app: &mut App| {
        home(app);
        app.online.account = Some(Profile {
            username: "ann-with-a-long-name".into(),
            ratings: ["bullet", "blitz", "rapid", "classical"]
                .into_iter()
                .map(|category| ShownRating {
                    key: RatingKey {
                        game: GameId::Chess,
                        category,
                    },
                    rating: 2999,
                    games: 1,
                    wins: 0,
                    provisional: true,
                })
                .collect(),
        });
        app.go_home(HomeItem::QuickPlay, None);
        app.handle(Key::Enter);
    };
    apps.push(app(ANN, &views[1], rated));
    let mut noted = app(ANN, &views[1], rated);
    noted.notify("Couldn't reach the matchmaker.".into());
    apps.push(noted);
    let mut waiting = app(ANN, &views[1], home);
    waiting.go_home(HomeItem::QuickPlay, None);
    waiting.handle(Key::Enter);
    waiting.handle(Key::Enter);
    assert_eq!(waiting.screen, Screen::Finding);
    apps.push(waiting);
    apps
}

fn draw_all(apps: &[App], sizes: &[(u16, u16)]) {
    for &(w, h) in sizes {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        for app in apps {
            terminal
                .draw(|frame| shell_tui::ui::draw(frame, app))
                .unwrap_or_else(|e| panic!("{:?} at {w}x{h}: {e}", app.screen));
        }
    }
}

#[test]
fn every_screen_draws_at_every_size() {
    let started = Instant::now();
    let apps = screens();
    let sizes: Vec<(u16, u16)> = (1..=120)
        .flat_map(|w| (1..=40).map(move |h| (w, h)))
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for chunk in sizes.chunks(sizes.len().div_ceil(threads)) {
            let apps = &apps;
            s.spawn(move || draw_all(apps, chunk));
        }
    });
    eprintln!(
        "drew {} screens at {} sizes in {:?}",
        apps.len(),
        sizes.len(),
        started.elapsed()
    );
}

#[test]
fn an_away_player_shows_how_long_their_seat_waits() {
    let mut view = playing(&["e2e4", "e7e5"]).view();
    view.shell.seats[1].away = Some(Duration::from_secs(75));
    let screen = shown(&app(ANN, &view, |_| {}), 80, 24);
    assert!(screen.contains("ben (away)"), "{screen}");
    assert!(
        screen.contains("ben is away · back within 1:15"),
        "{screen}"
    );
}

#[test]
fn the_board_fits_80_by_24_with_its_panel() {
    let app = app(ANN, &playing(&["e2e4", "e7e5"]).view(), |_| {});
    let screen = shown(&app, 80, 24);
    // The board's files, and the panel beside it with both clocks.
    assert!(
        screen.contains("a    b    c    d    e    f    g    h"),
        "{screen}"
    );
    assert!(screen.contains("1. e4      e5"), "{screen}");
    assert_eq!(screen.matches("5:0").count(), 2, "{screen}");
    let small = shown(&app, 24, 9);
    assert!(small.contains("Make the terminal"), "{small}");
}
