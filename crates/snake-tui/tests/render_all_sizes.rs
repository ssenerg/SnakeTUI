//! Players' terminals can be any size, and over SSH a panic while drawing
//! would drop the connection, so every screen must draw at every size.

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Size;
use snake_core::config::{GameConfig, Rules};
use snake_core::records::Records;
use snake_core::solo::{GameOverReason, SoloGame};
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
