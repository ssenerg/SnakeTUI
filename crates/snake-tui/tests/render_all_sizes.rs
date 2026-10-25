//! Players' terminals can be any size, and over SSH a panic while drawing
//! would drop the connection, so every screen must draw at every size.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Size;
use snake_core::config::{GameConfig, Rules};
use snake_core::records::Records;
use snake_core::round::RatingChange;
use snake_core::round::{
    Entrant, Outcome, PhaseView, RoomKind, RoomView, Round, RoundSettings, quick_rules,
};
use snake_core::solo::{GameOverReason, SoloGame};
use snake_tui::online::{Field, FormKind, FriendRow, FriendsView, KeyRow, Profile, QueueRegion};
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
    // Every room screen again, with round trips from fast to awful, and
    // some players not timed.
    let timed: Vec<RoomView> = views
        .iter()
        .map(|view| {
            let mut view = view.clone();
            for (i, seat) in view.seats.iter_mut().enumerate() {
                let ms = [1, 45, 120, 999, 1500, 0][i % 6];
                seat.ping = (ms > 0).then(|| Duration::from_millis(ms));
            }
            view
        })
        .collect();
    views.extend(timed);
    views
}

fn online_screens(rules: &Rules) -> Vec<App> {
    let cfg = GameConfig {
        rules: rules.clone(),
        ..GameConfig::default()
    };
    let new_app = |screen: Screen| {
        let mut app = App::online(
            cfg.clone(),
            Records::default(),
            "~player-0123456789".into(),
            true,
        );
        app.screen = screen;
        app
    };
    // Signed in, with friends, requests and remembered machines.
    let account = |screen: Screen| {
        let mut app = new_app(screen);
        app.set_profile(Some(Profile {
            username: "player0123456789".into(),
            rating: 1043,
            games: 12,
            wins: 4,
        }));
        let online = app.online.as_mut().unwrap();
        online.requests = 2;
        online.keys = (0..10)
            .map(|i| KeyRow {
                fingerprint: format!("SHA256:{i}abcdefghijklmnopqrstuvwxyz0123456789"),
                last_used: "used 3 days ago".into(),
                this_machine: i == 0,
            })
            .collect();
        online.friends = FriendsView {
            friends: (0..20)
                .map(|i| FriendRow {
                    name: format!("friend-number-{i:02}"),
                    status: if i % 2 == 0 {
                        "in a private room".into()
                    } else {
                        "last seen 12 days ago".into()
                    },
                    online: i % 2 == 0,
                })
                .collect(),
            incoming: vec!["someone-asking".into(), "another".into()],
            outgoing: vec!["waiting-on-them".into()],
        };
        app
    };
    let guest_items = new_app(Screen::Home { selected: 0 }).home_items().len();
    let mut apps: Vec<App> = (0..guest_items)
        .map(|selected| new_app(Screen::Home { selected }))
        .collect();
    let account_items = account(Screen::Home { selected: 0 }).home_items().len();
    apps.extend((0..account_items).map(|selected| account(Screen::Home { selected })));
    for selected in [0, 3, 10, 12] {
        apps.push(account(Screen::Account { selected }));
    }
    for selected in [0, 2, 5, 23] {
        apps.push(account(Screen::Friends { selected }));
    }
    for kind in [FormKind::SignUp, FormKind::SignIn, FormKind::ChangePassword] {
        let mut app = if kind == FormKind::ChangePassword {
            account(Screen::Form(kind))
        } else {
            new_app(Screen::Form(kind))
        };
        apps.push(new_app(Screen::Form(kind)));
        app.form.username = "a-sixteen-letter".into();
        app.form.focus = 1;
        app.form.error = Some("That username is reserved. Pick another.".into());
        apps.push(app);
    }
    let mut busy = new_app(Screen::Form(FormKind::SignIn));
    busy.form.busy = true;
    apps.push(busy);
    let mut invited = account(Screen::Home { selected: 0 });
    invited.invited("someone-with-a-long-name".into());
    apps.push(invited);
    let mut adding = account(Screen::Input(Field::FriendName));
    adding.input.text = "bob".into();
    apps.push(adding);
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
    // In a cluster, after the queue has opened to every region.
    let mut regional = new_app(Screen::Finding);
    regional.set_region(QueueRegion {
        name: "eu-west-somewhere-long".into(),
        widens_after: Duration::from_secs(20),
    });
    if let Some(online) = regional.online.as_mut() {
        online.waited = Duration::from_secs(95);
        online.queue_len = 1234;
    }
    apps.push(regional);
    apps.push(new_app(Screen::Joining { by_code: true }));
    let views = room_views(rules);
    for view in &views {
        let mut app = new_app(Screen::Room);
        app.enter_room(2, Arc::new(view.clone()));
        apps.push(app);
    }
    // Results of a rated match, with ratings up and down.
    let mut rated = views
        .iter()
        .rfind(|v| matches!(v.phase, PhaseView::Results { .. }))
        .expect("results")
        .clone();
    rated.ratings = rated
        .placements
        .iter()
        .enumerate()
        .map(|(i, p)| RatingChange {
            id: p.who.id,
            before: 1000,
            after: if i == 0 { 1031 } else { 988 },
        })
        .collect();
    let mut app = account(Screen::Room);
    app.enter_room(2, Arc::new(rated));
    apps.push(app);
    // A private lobby, signed in, with a notice.
    let lobby = views
        .iter()
        .find(|v| v.code.is_some())
        .expect("a private lobby")
        .clone();
    let mut app = account(Screen::Room);
    app.enter_room(1, Arc::new(lobby));
    app.notify("Invited friend-number-00.".into());
    apps.push(app);
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

/// What a `w`×`h` terminal shows of `app`.
fn shown(app: &App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn round_trips_show_by_each_player() {
    let ms = Duration::from_millis;
    let short = |id| Entrant {
        id,
        name: format!("~p{id}"),
    };
    let timed = |mut view: RoomView| {
        for seat in &mut view.seats {
            seat.ping = match seat.who.id {
                1 => Some(ms(45)),
                2 => Some(ms(1500)),
                _ => None,
            };
        }
        Arc::new(view)
    };
    let settings = RoundSettings::new(Rules::default(), RoomKind::Private).unwrap();
    let mut round = Round::new(settings, 1);
    for id in 1..=3 {
        round.join(short(id)).unwrap();
    }
    let mut app = App::online(
        GameConfig::default(),
        Records::default(),
        "~p1".into(),
        true,
    );
    app.screen = Screen::Room;
    app.enter_room(1, timed(round.view()));
    let lobby = shown(&app, 80, 24);
    assert!(lobby.contains("~p1 (you) · 45 ms"), "{lobby}");
    assert!(lobby.contains("~p2 · 1.5 s"), "{lobby}");
    assert!(!lobby.contains("~p3 ·"), "not timed yet:\n{lobby}");

    for id in 1..=3 {
        round.toggle_ready(id);
    }
    while !matches!(round.view().phase, PhaseView::Running { .. }) {
        round.advance(ms(100));
    }
    app.update_room(timed(round.view()));
    // A wide scoreboard has room for the numbers; at 80 columns, dots.
    let wide = shown(&app, 100, 24);
    assert!(wide.contains("45ms") && wide.contains("1.5s"), "{wide}");
    let narrow = shown(&app, 80, 24);
    assert!(narrow.contains('●') && !narrow.contains("45ms"), "{narrow}");
}

#[test]
fn only_the_host_sees_how_to_change_the_speed() {
    let settings = RoundSettings::new(Rules::default(), RoomKind::Private).unwrap();
    let mut round = Round::new(settings, 1);
    for id in 1..=2 {
        let name = format!("~p{id}");
        round.join(Entrant { id, name }).unwrap();
    }
    let lobby_of = |me| {
        let mut app = App::online(
            GameConfig::default(),
            Records::default(),
            format!("~p{me}"),
            true,
        );
        app.screen = Screen::Room;
        app.enter_room(me, Arc::new(round.view()));
        shown(&app, 80, 24)
    };
    let host = lobby_of(1);
    assert!(host.contains("◂ 150 ms ticks ▸"), "{host}");
    assert!(host.contains("←→ speed"), "{host}");
    let guest = lobby_of(2);
    assert!(
        guest.contains("150 ms ticks") && !guest.contains('◂'),
        "{guest}"
    );
    assert!(!guest.contains("speed"), "{guest}");
}
