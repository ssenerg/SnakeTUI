//! Players' terminals can be any size, and over SSH a panic while drawing
//! would drop the connection, so every screen must draw at every size.

use std::sync::Arc;
use std::time::{Duration, Instant};

use game_api::GameId;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Size;
use shell_tui::app::BOTS_OFFERED_AFTER;
use shell_tui::online::{
    Field, FormKind, FriendRow, FriendsView, KeyRow, LeaderRow, LeaderboardView, MyPlace, Profile,
    QueueRegion, ShownRating,
};
use shell_tui::{App, Screen};
use snake_core::config::{GameConfig, Rules};
use snake_core::records::Records;
use snake_core::round::RatingChange;
use snake_core::round::{
    Entrant, PhaseView, QUICK_RATING, Room, RoomKind, RoomSettings, RoomView, bot_entrant,
    quick_rules,
};
use snake_core::solo::{GameOverReason, SoloGame};
use snake_tui::{SnakeScreen, SnakeUi, ui};

/// The app with Snake as its game, with these settings, as the guest
/// `name`.
fn app_with(cfg: GameConfig, name: &str) -> App {
    let snake = SnakeUi::new(cfg, Records::default());
    App::online(vec![Box::new(snake)], None, name.into(), true)
}

fn snake(app: &mut App) -> &mut SnakeUi {
    app.game_ui_mut().expect("Snake")
}

/// A Snake quick-play rating as an account shows it.
fn snake_rating(rating: u32, games: u32, wins: u32) -> ShownRating {
    ShownRating {
        key: QUICK_RATING,
        rating,
        games,
        wins,
    }
}

/// A leaderboard of `len` players with long names, and `me`.
fn leaderboard(len: u32, me: MyPlace) -> LeaderboardView {
    let rows = (1..=len)
        .map(|place| LeaderRow {
            place,
            name: format!("player-number-{place:02}"),
            rating: 1500 - place * 10,
            games: 100 + place,
            wins: 50,
        })
        .collect();
    LeaderboardView {
        key: QUICK_RATING,
        rows,
        me,
    }
}

fn ranked(place: u32) -> MyPlace {
    MyPlace::Ranked(LeaderRow {
        place,
        name: "player0123456789".into(),
        rating: 988,
        games: 6,
        wins: 1,
    })
}

/// One app per screen worth drawing, on a board of `rules`.
fn screens(rules: &Rules) -> Vec<App> {
    let cfg = GameConfig {
        rules: rules.clone(),
        ..GameConfig::default()
    };
    let new_app = |screen: SnakeScreen| {
        let mut app = app_with(cfg.clone(), "~player-0123456789");
        app.screen = Screen::Game;
        let snake = snake(&mut app);
        snake.screen = screen;
        if screen != SnakeScreen::Menu {
            snake.game = Some(SoloGame::new(rules.clone()).unwrap());
        }
        app
    };

    let mut apps = vec![new_app(SnakeScreen::Menu)];
    let mut confirming = new_app(SnakeScreen::Menu);
    let snake_ui = snake(&mut confirming);
    snake_ui.confirm_reset = true;
    snake_ui.selected = snake_core::settings::SETTINGS.len() - 1;
    apps.push(confirming);

    apps.push(new_app(SnakeScreen::Playing));
    let mut started = new_app(SnakeScreen::Playing);
    snake(&mut started).game.as_mut().unwrap().started = true;
    apps.push(started);
    // Every kind of food on the board, without moving the snake.
    let mut with_food = new_app(SnakeScreen::Playing);
    let game = snake(&mut with_food).game.as_mut().unwrap();
    game.started = true;
    game.world.advance_clock(Duration::from_secs(20));
    assert!(game.world.foods.len() >= 3);
    apps.push(with_food);

    for selected in [0, 1] {
        apps.push(new_app(SnakeScreen::Paused { selected }));
    }
    for reason in [
        GameOverReason::HitWall,
        GameOverReason::HitSelf,
        GameOverReason::Poisoned,
        GameOverReason::MaxLength,
    ] {
        for new_record in [false, true] {
            let mut app = new_app(SnakeScreen::GameOver { new_record });
            snake(&mut app).game.as_mut().unwrap().over = Some(reason);
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
    if let Some(settings) = RoomSettings::new(rules.clone(), RoomKind::Private) {
        let mut round = Room::new(settings.clone(), 1);
        round.join(entrant(1)).unwrap();
        views.push(round.view());
        for id in 2..=settings.max_players as u64 {
            round.join(entrant(id)).unwrap();
        }
        round.toggle_ready(2);
        let mut lobby = round.view();
        lobby.shell.code = Some("K7M2QX".into());
        views.push(lobby);
        for id in 1..=settings.max_players as u64 {
            round.toggle_ready(id);
        }
        views.push(round.view());
        while !matches!(round.view().shell.phase, PhaseView::Results { .. }) {
            round.advance(step);
            if matches!(round.view().shell.phase, PhaseView::Running { .. }) && views.len() < 5 {
                views.push(round.view());
            }
        }
        views.push(round.view());
    }
    // A private lobby full of bots, and a match against bots, played out.
    if let Some(settings) = RoomSettings::new(rules.clone(), RoomKind::Private) {
        let mut round = Room::new(settings, 1);
        round.join(entrant(1)).unwrap();
        while round.add_bot(1) {}
        views.push(round.view());
    }
    let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
    let bots = (0..3).map(bot_entrant);
    let mut round = Room::new_match(
        settings.clone(),
        3,
        [entrant(1)].into_iter().chain(bots).collect(),
    );
    while !matches!(round.view().shell.phase, PhaseView::Results { .. }) {
        round.advance(step);
    }
    views.push(round.view());

    let mut round = Room::new_match(settings, 2, (1..=8).map(entrant).collect());
    views.push(round.view());
    for _ in 0..40 {
        round.advance(step);
    }
    round.leave(1);
    views.push(round.view());
    while !matches!(round.view().shell.phase, PhaseView::Results { .. }) {
        round.advance(step);
    }
    let over = round.view();
    // Player 1 or 2 wins, alone or tied with the next, in time or not.
    for (first, tied, time_up) in [(1, false, false), (2, false, true), (1, true, true)] {
        let mut view = over.clone();
        let placements = &mut view.shell.placements;
        placements.sort_by_key(|p| p.who.id != first);
        for (i, p) in placements.iter_mut().enumerate() {
            p.place = if i == 0 || (tied && i == 1) { 1 } else { i + 1 };
        }
        view.shell.phase = PhaseView::Results { next_in: None };
        view.game.time_up = time_up;
        views.push(view);
    }
    // Every room screen again, with round trips from fast to awful, and
    // some players not timed.
    let timed: Vec<RoomView> = views
        .iter()
        .map(|view| {
            let mut view = view.clone();
            for (i, seat) in view.shell.seats.iter_mut().enumerate() {
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
        let mut app = app_with(cfg.clone(), "~player-0123456789");
        app.screen = screen;
        app
    };
    // Signed in, with friends, requests and remembered machines.
    let account = |screen: Screen| {
        let mut app = new_app(screen);
        app.set_profile(Some(Profile {
            username: "player0123456789".into(),
            ratings: vec![snake_rating(1043, 12, 4)],
        }));
        let online = &mut app.online;
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
                    watch: i % 4 == 0,
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
    // The picker, and with something to say.
    apps.push(new_app(Screen::Picker { selected: 0 }));
    let mut picking = new_app(Screen::Picker { selected: 0 });
    picking.notify("Pick one to play.".into());
    apps.push(picking);
    let account_items = account(Screen::Home { selected: 0 }).home_items().len();
    apps.extend((0..account_items).map(|selected| account(Screen::Home { selected })));
    for selected in [0, 3, 10, 12] {
        apps.push(account(Screen::Account { selected }));
    }
    for selected in [0, 2, 5, 23] {
        apps.push(account(Screen::Friends { selected }));
    }
    apps.push(account(Screen::Leaderboard { category: 0 }));
    for (signed_in, view) in [
        (false, leaderboard(0, MyPlace::Guest)),
        (false, leaderboard(20, MyPlace::Guest)),
        (true, leaderboard(20, ranked(3))),
        (true, leaderboard(20, ranked(37))),
        (true, leaderboard(4, MyPlace::Unranked { games_left: 1 })),
    ] {
        let mut app = if signed_in {
            account(Screen::Leaderboard { category: 0 })
        } else {
            new_app(Screen::Leaderboard { category: 0 })
        };
        app.set_leaderboard(view);
        apps.push(app);
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
    invited.invited("someone-with-a-long-name".into(), GameId::Snake);
    apps.push(invited);
    let mut adding = account(Screen::Input(Field::FriendName));
    adding.input.text = "bob".into();
    apps.push(adding);
    let mut notice = new_app(Screen::Home { selected: 1 });
    notice.online.notice = Some(
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
    // Long enough for bots to be offered.
    let mut bored = new_app(Screen::Finding);
    bored.online.waited = BOTS_OFFERED_AFTER;
    apps.push(bored);
    // In a cluster, after the queue has opened to every region.
    let mut regional = new_app(Screen::Finding);
    regional.set_region(QueueRegion {
        name: "eu-west-somewhere-long".into(),
        widens_after: Duration::from_secs(20),
    });
    regional.online.waited = Duration::from_secs(95);
    regional.online.queue_len = 1234;
    apps.push(regional);
    apps.push(new_app(Screen::Joining { by_code: true }));
    let views = room_views(rules);
    for view in &views {
        let mut app = new_app(Screen::Room);
        app.enter_room(2, Arc::new(view.clone()));
        apps.push(app);
    }
    // Watching player 2 in each phase, with others watching too.
    let mut phases = Vec::new();
    for view in &views {
        let phase = std::mem::discriminant(&view.shell.phase);
        if phases.contains(&phase) {
            continue;
        }
        phases.push(phase);
        let mut watched = view.clone();
        watched.shell.watchers = 3;
        let mut app = account(Screen::JoiningToWatch);
        app.watch_room(99, entrant(2).name, Arc::new(watched));
        apps.push(app);
    }
    apps.push(account(Screen::JoiningToWatch));
    // Results of a rated match, with ratings up and down.
    let mut rated = views
        .iter()
        .rfind(|v| matches!(v.shell.phase, PhaseView::Results { .. }))
        .expect("results")
        .clone();
    rated.shell.ratings = rated
        .shell
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
        .find(|v| v.shell.code.is_some())
        .expect("a private lobby")
        .clone();
    let mut app = account(Screen::Room);
    app.enter_room(1, Arc::new(lobby.clone()));
    app.notify("Invited friend-number-00.".into());
    apps.push(app);
    // Its rules: the host's, at the top, the bottom and in between, and
    // someone else's, told what the host changed.
    let host = lobby.shell.host().expect("a host").who.id;
    let rules_of = |mut app: App, selected| {
        app.screen = Screen::Game;
        snake(&mut app).screen = SnakeScreen::RoomRules { selected };
        app
    };
    for selected in [0, 7, 13] {
        let mut app = new_app(Screen::Room);
        app.enter_room(host, Arc::new(lobby.clone()));
        apps.push(rules_of(app, selected));
    }
    let mut app = account(Screen::Room);
    app.enter_room(host + 1000, Arc::new(lobby.clone()));
    let mut app = rules_of(app, 13);
    app.notify("~player-0123456789 set super food lifetime to 120 s.".into());
    apps.push(app);
    // While a round runs: as one who left, as someone watching, and asking
    // to leave.
    let running = views
        .iter()
        .find(|v| matches!(v.shell.phase, PhaseView::Running { .. }))
        .expect("a running round");
    for me in [1, 99] {
        let mut app = new_app(Screen::Room);
        app.enter_room(me, Arc::new(running.clone()));
        apps.push(app);
    }
    let mut leaving = new_app(Screen::Room);
    leaving.enter_room(2, Arc::new(running.clone()));
    leaving.online.room.as_mut().unwrap().confirm_leave = true;
    apps.push(leaving);
    apps
}

fn draw_all(apps: &[App], sizes: &[(u16, u16)]) {
    for &(w, h) in sizes {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
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
    terminal
        .draw(|frame| shell_tui::ui::draw(frame, app))
        .unwrap();
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
        for seat in &mut view.shell.seats {
            seat.ping = match seat.who.id {
                1 => Some(ms(45)),
                2 => Some(ms(1500)),
                _ => None,
            };
        }
        Arc::new(view)
    };
    let settings = RoomSettings::new(Rules::default(), RoomKind::Private).unwrap();
    let mut round = Room::new(settings, 1);
    for id in 1..=3 {
        round.join(short(id)).unwrap();
    }
    let mut app = app_with(GameConfig::default(), "~p1");
    app.screen = Screen::Room;
    app.enter_room(1, timed(round.view()));
    let lobby = shown(&app, 80, 24);
    assert!(lobby.contains("~p1 (you) · 45 ms"), "{lobby}");
    assert!(lobby.contains("~p2 · 1.5 s"), "{lobby}");
    assert!(!lobby.contains("~p3 ·"), "not timed yet:\n{lobby}");

    for id in 1..=3 {
        round.toggle_ready(id);
    }
    while !matches!(round.view().shell.phase, PhaseView::Running { .. }) {
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
    let settings = RoomSettings::new(Rules::default(), RoomKind::Private).unwrap();
    let mut round = Room::new(settings, 1);
    for id in 1..=2 {
        let name = format!("~p{id}");
        round.join(Entrant { id, name }).unwrap();
    }
    let lobby_of = |me| {
        let mut app = app_with(GameConfig::default(), &format!("~p{me}"));
        app.screen = Screen::Room;
        app.enter_room(me, Arc::new(round.view()));
        shown(&app, 80, 24)
    };
    let host = lobby_of(1);
    assert!(host.contains("◂ 150 ms ticks ▸"), "{host}");
    assert!(host.contains("Enter ready · e rules"), "{host}");
    assert!(host.contains("←→ speed · b add a bot"), "{host}");
    let guest = lobby_of(2);
    assert!(
        guest.contains("150 ms ticks") && !guest.contains('◂'),
        "{guest}"
    );
    assert!(!guest.contains("speed"), "{guest}");
    assert!(guest.contains("e rules"), "everyone can read them: {guest}");

    let rules_of = |me| {
        let mut app = app_with(GameConfig::default(), &format!("~p{me}"));
        app.enter_room(me, Arc::new(round.view()));
        app.screen = Screen::Game;
        snake(&mut app).screen = SnakeScreen::RoomRules { selected: 0 };
        shown(&app, 80, 24)
    };
    let host = rules_of(1);
    assert!(host.contains("Room rules"), "{host}");
    assert!(host.contains("◂ 30 ▸"), "the width, to change: {host}");
    assert!(
        host.contains("super food growth"),
        "every rule fits: {host}"
    );
    assert!(
        !host.contains("move timing") && !host.contains("icon"),
        "{host}"
    );
    assert!(host.contains("Room for 8 players"), "{host}");
    let guest = rules_of(2);
    assert!(
        guest.contains("❯ board width") && !guest.contains('◂'),
        "{guest}"
    );
    assert!(guest.contains("Only ~p1 can change these."), "{guest}");
}

#[test]
fn the_leaderboard_shows_what_fits_and_always_your_place() {
    let app = |view: Option<LeaderboardView>| {
        let mut app = app_with(GameConfig::default(), "~guest");
        app.screen = Screen::Leaderboard { category: 0 };
        if let Some(view) = view {
            app.set_leaderboard(view);
        }
        app
    };
    let loading = shown(&app(None), 80, 24);
    assert!(loading.contains("Loading…"), "{loading}");

    let empty = shown(&app(Some(leaderboard(0, MyPlace::Guest))), 80, 24);
    assert!(
        empty.contains("Nobody has played 5 rated games yet."),
        "{empty}"
    );
    assert!(empty.contains("Sign up to get a rating."), "{empty}");

    let full = shown(&app(Some(leaderboard(20, ranked(37)))), 80, 24);
    assert!(full.contains("#  Player"), "{full}");
    assert!(
        full.contains(" 1  player-number-01    1490    101    50"),
        "{full}"
    );
    assert!(full.contains("player-number-19"), "19 rows fit: {full}");
    assert!(!full.contains("player-number-20"), "{full}");
    assert!(full.contains("You: 37th · 988 · 6 games"), "{full}");
    let tall = shown(&app(Some(leaderboard(20, ranked(37)))), 80, 26);
    assert!(tall.contains("player-number-20"), "{tall}");

    let unranked = MyPlace::Unranked { games_left: 2 };
    let unranked = shown(&app(Some(leaderboard(3, unranked))), 80, 24);
    assert!(
        unranked.contains("2 more rated games to be ranked."),
        "{unranked}"
    );
}

#[test]
fn bots_are_marked_and_offered() {
    let app_at = |screen: Screen| {
        let mut app = app_with(GameConfig::default(), "~p1");
        app.screen = screen;
        app
    };
    let settings = RoomSettings::new(Rules::default(), RoomKind::Private).unwrap();
    let mut round = Room::new(settings, 1);
    round.join(entrant(1)).unwrap();
    round.add_bot(1);
    round.add_bot(1);
    let mut host = app_at(Screen::Room);
    host.enter_room(1, Arc::new(round.view()));
    let lobby = shown(&host, 80, 24);
    assert!(lobby.contains("✓ Viper (bot)"), "always ready: {lobby}");
    assert!(lobby.contains("Cobra (bot)"), "{lobby}");
    assert!(lobby.contains("b add a bot · x take one out"), "{lobby}");

    let mut waiting = app_at(Screen::Finding);
    assert!(!shown(&waiting, 80, 24).contains("bots"));
    waiting.online.waited = BOTS_OFFERED_AFTER;
    let offer = shown(&waiting, 80, 24);
    assert!(offer.contains("Play bots now, unrated."), "{offer}");
    assert!(offer.contains("b play bots · Esc cancel"), "{offer}");
}

#[test]
fn watchers_follow_their_friend_and_are_counted() {
    let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
    let players = vec![
        Entrant {
            id: 1,
            name: "~p1".into(),
        },
        Entrant {
            id: 2,
            name: "bob".into(),
        },
    ];
    let mut round = Room::new_match(settings, 1, players);
    while !matches!(round.view().shell.phase, PhaseView::Running { .. }) {
        round.advance(Duration::from_millis(100));
    }
    let mut view = round.view();
    view.shell.watchers = 2;
    let mut app = app_with(GameConfig::default(), "~me");
    app.screen = Screen::JoiningToWatch;
    app.watch_room(99, "bob".into(), Arc::new(view.clone()));
    let watching = shown(&app, 80, 24);
    assert!(watching.contains("Watching bob · Esc leave"), "{watching}");
    assert!(watching.contains("2 watching"), "{watching}");
    assert!(!watching.contains("←↑↓→ move"), "{watching}");

    // Players see who's watching too.
    let mut player = app_with(GameConfig::default(), "~p1");
    player.enter_room(1, Arc::new(view));
    let playing = shown(&player, 80, 24);
    assert!(playing.contains("2 watching"), "{playing}");
}
