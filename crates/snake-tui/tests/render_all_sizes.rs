//! Players' terminals can be any size, and over SSH a panic while drawing
//! would drop the connection, so every screen must draw at every size.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Size;
use snake_core::config::{GameConfig, Rules};
use snake_core::records::Records;
use snake_core::round::{
    Entrant, Outcome, PhaseView, RoomKind, RoomView, Round, RoundSettings, quick_rules,
};
use snake_core::solo::{GameOverReason, SoloGame};
use snake_tui::app::{Field, HOME};
use snake_tui::{App, Screen, ui};

/// One app per screen worth drawing, on a board of `rules`.
fn screens(rules: &Rules) -> Vec<App> {
    let cfg = GameConfig {
        rules: rules.clone(),
        ..GameConfig::default()
    };
    let new_app = |screen: Screen| {
        let mut app = App::new(cfg.clone(), Records::default());
        app.screen = screen;
        if screen != Screen::Menu {
            app.game = Some(SoloGame::new(rules.clone()).unwrap());
        }
        app
    };

    let mut apps = vec![new_app(Screen::Menu)];
    let mut confirming = new_app(Screen::Menu);
    confirming.confirm_reset = true;
    confirming.selected = snake_core::settings::SETTINGS.len() - 1;
    apps.push(confirming);

    apps.push(new_app(Screen::Playing));
    let mut started = new_app(Screen::Playing);
    started.game.as_mut().unwrap().started = true;
    apps.push(started);
    // Every kind of food on the board, without moving the snake.
    let mut with_food = new_app(Screen::Playing);
    let game = with_food.game.as_mut().unwrap();
    game.started = true;
    game.world.advance_clock(Duration::from_secs(20));
    assert!(game.world.foods.len() >= 3);
    apps.push(with_food);

    for selected in [0, 1] {
        apps.push(new_app(Screen::Paused { selected }));
    }
    for reason in [
        GameOverReason::HitWall,
        GameOverReason::HitSelf,
        GameOverReason::Poisoned,
        GameOverReason::MaxLength,
    ] {
        for new_record in [false, true] {
            let mut app = new_app(Screen::GameOver { new_record });
            app.game.as_mut().unwrap().over = Some(reason);
            apps.push(app);
        }
    }
    apps.extend(online_screens(rules));
    apps
}

fn entrant(id: u64) -> Entrant {
    Entrant {
        id,
        name: format!("~player-with-a-long-name{id}"),
    }
}

/// A room's view at each phase worth drawing, for private rooms of `rules`
/// and for matches.
fn room_views(rules: &Rules) -> Vec<RoomView> {
    let mut views = Vec::new();
    let step = Duration::from_millis(100);
    if let Some(settings) = RoundSettings::new(rules.clone(), RoomKind::Private) {
        let mut round = Round::new(settings.clone(), 1);
        round.join(entrant(1)).unwrap();
        views.push(round.view());
        for id in 2..=settings.max_players as u64 {
            round.join(entrant(id)).unwrap();
        }
        round.toggle_ready(2);
        let mut lobby = round.view();
        lobby.code = Some("K7M2QX".into());
        views.push(lobby);
        for id in 1..=settings.max_players as u64 {
            round.toggle_ready(id);
        }
        views.push(round.view());
        while !matches!(round.view().phase, PhaseView::Results { .. }) {
            round.advance(step);
            if matches!(round.view().phase, PhaseView::Running { .. }) && views.len() < 5 {
                views.push(round.view());
            }
        }
        views.push(round.view());
    }
    let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
    let mut round = Round::new_match(settings, 2, (1..=8).map(entrant).collect());
    views.push(round.view());
    for _ in 0..40 {
        round.advance(step);
    }
    round.leave(1);
    views.push(round.view());
    while !matches!(round.view().phase, PhaseView::Results { .. }) {
        round.advance(step);
    }
    let over = round.view();
    for outcome in [
        Outcome::Won {
            winner: entrant(1),
            time_up: false,
        },
        Outcome::Won {
            winner: entrant(2),
            time_up: true,
        },
        Outcome::Draw { time_up: true },
    ] {
        let mut view = over.clone();
        view.phase = PhaseView::Results {
            outcome,
            next_in: None,
        };
        views.push(view);
    }
    views
}

fn online_screens(rules: &Rules) -> Vec<App> {
    let cfg = GameConfig {
        rules: rules.clone(),
        ..GameConfig::default()
    };
    let new_app = |screen: Screen| {
        let mut app = App::online(cfg.clone(), Records::default(), "~player-0123456789".into());
        app.screen = screen;
        app
    };
    let mut apps: Vec<App> = (0..HOME.len())
        .map(|selected| new_app(Screen::Home { selected }))
        .collect();
    let mut notice = new_app(Screen::Home { selected: 1 });
    notice.online.as_mut().unwrap().notice = Some(
        "Your terminal is 60×20; matches need 80×21 to show the whole board. \
         Press Enter again to play anyway."
            .into(),
    );
    apps.push(notice);
    for field in [Field::Code, Field::Nickname] {
        let mut app = new_app(Screen::Input(field));
        app.input.text = "ABC123".into();
        app.input.error = Some("No room has that code.".into());
        apps.push(app);
    }
    apps.push(new_app(Screen::Finding));
    apps.push(new_app(Screen::Joining { by_code: true }));
    let views = room_views(rules);
    for view in &views {
        let mut app = new_app(Screen::Room);
        app.enter_room(2, Arc::new(view.clone()));
        apps.push(app);
    }
    // While a round runs: as one who left, as someone watching, and asking
    // to leave.
    let running = views
        .iter()
        .find(|v| matches!(v.phase, PhaseView::Running { .. }))
        .expect("a running round");
    for me in [1, 99] {
        let mut app = new_app(Screen::Room);
        app.enter_room(me, Arc::new(running.clone()));
        apps.push(app);
    }
    let mut leaving = new_app(Screen::Room);
    leaving.enter_room(2, Arc::new(running.clone()));
    leaving
        .online
        .as_mut()
        .unwrap()
        .room
        .as_mut()
        .unwrap()
        .confirm_leave = true;
    apps.push(leaving);
    apps
}

fn draw_all(apps: &[App], sizes: &[(u16, u16)]) {
    for &(w, h) in sizes {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        for app in apps {
            terminal
                .draw(|frame| ui::draw(frame, app))
                .unwrap_or_else(|e| panic!("{:?} at {w}x{h}: {e}", app.screen));
        }
    }
}

#[test]
fn every_screen_draws_at_every_size() {
    let started = Instant::now();
    let apps = screens(&Rules::default());
    let sizes: Vec<(u16, u16)> = (1..=120)
        .flat_map(|w| (1..=40).map(move |h| (w, h)))
        .collect();
    // Exhaustive, so spread it over the cores to keep `cargo test` quick.
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
fn smallest_and_largest_boards_draw() {
    for (width, height) in [(10, 8), (80, 40)] {
        let rules = Rules {
            width,
            height,
            ..Rules::default()
        };
        // Two columns per cell, plus border and padding.
        let (need_w, need_h) = (width * 2 + 6, height + 4);
        assert!(ui::board_fits(width, height, Size::new(need_w, need_h)));
        assert!(!ui::board_fits(
            width,
            height,
            Size::new(need_w - 1, need_h)
        ));
        assert!(!ui::board_fits(
            width,
            height,
            Size::new(need_w, need_h - 1)
        ));

        let sizes = [
            (need_w - 1, need_h),
            (need_w, need_h - 1),
            (need_w, need_h),
            (200, 60),
        ];
        draw_all(&screens(&rules), &sizes);
    }
}
