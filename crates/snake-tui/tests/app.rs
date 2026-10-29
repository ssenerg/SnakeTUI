//! The app with Snake as its game: the home menu, rooms, accounts and
//! friends, as a player's keys drive them.

use std::sync::Arc;
use std::time::Duration;

use game_api::GameId;
use game_api::codec::{decode, encode};
use ratatui::layout::Size;
use shell_tui::app::BOTS_OFFERED_AFTER;
use shell_tui::online::{
    Field, FormKind, FormRow, FriendRow, FriendsView, HomeItem, LeaderboardView, MyPlace, Profile,
    ShownRating,
};
use shell_tui::{App, Effect, GameFx, Key, Screen};
use snake_core::config::{Rules, TICK_MS_MAX, TICK_MS_MIN};
use snake_core::geom::Direction;
use snake_core::round::{
    Entrant, PhaseView, QUICK_POOL, QUICK_RATING, Room, RoomKind, RoomSettings, RoomView,
    max_players_for, quick_rules,
};
use snake_tui::{SnakeScreen, SnakeUi};

fn steer(dir: Direction) -> Effect {
    Effect::Game(GameId::Snake, GameFx::Play(encode(&dir)))
}

fn set_rules(rules: Rules) -> Effect {
    Effect::Game(GameId::Snake, GameFx::Settings(encode(&rules)))
}

fn quick_play() -> Effect {
    Effect::QuickPlay {
        game: GameId::Snake,
        pool: QUICK_POOL,
    }
}

/// A Snake quick-play rating as an account shows it.
fn snake_rating(rating: u32, games: u32, wins: u32) -> ShownRating {
    ShownRating {
        key: QUICK_RATING,
        rating,
        games,
        wins,
        provisional: false,
    }
}

fn snake(app: &App) -> &SnakeUi {
    app.game_ui().expect("Snake")
}

fn online() -> App {
    let snake = Box::new(SnakeUi::default());
    let mut app = App::online(vec![snake], None, "~alice".into(), true);
    app.resize(Size::new(120, 40));
    app
}

fn signed_in() -> App {
    let mut app = online();
    app.set_profile(Some(Profile {
        username: "alice".into(),
        ratings: vec![snake_rating(1000, 0, 0)],
    }));
    app
}

fn choose(app: &mut App, item: HomeItem) -> Vec<Effect> {
    let at = app.home_items().iter().position(|i| *i == item).unwrap();
    app.screen = Screen::Home { selected: at };
    app.handle(Key::Enter)
}

fn type_in(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle(Key::Char(c));
    }
}

#[test]
fn online_single_player_goes_back_home_instead_of_quitting() {
    let mut app = online();
    app.handle(Key::Enter);
    assert_eq!(app.screen, Screen::Game);
    assert_eq!(snake(&app).screen, SnakeScreen::Menu);
    assert_eq!(app.handle(Key::Char('q')), []);
    assert_eq!(app.screen, Screen::Home { selected: 0 });
    assert_eq!(app.handle(Key::Char('q')), [Effect::Quit]);
}

#[test]
fn codes_are_typed_in_capitals_and_must_be_complete() {
    let mut app = online();
    choose(&mut app, HomeItem::JoinCode);
    assert_eq!(app.screen, Screen::Input(Field::Code));
    type_in(&mut app, "ab-c");
    assert_eq!(app.input.text, "ABC");
    app.handle(Key::Enter);
    assert!(app.input.error.is_some());
    type_in(&mut app, "d2k9");
    assert_eq!(app.input.text, "ABCD2K", "six at most");
    assert_eq!(app.handle(Key::Enter), [Effect::JoinRoom("ABCD2K".into())]);
    assert_eq!(app.screen, Screen::Joining { by_code: true });
    app.join_failed("No room has that code.".into());
    assert_eq!(app.screen, Screen::Input(Field::Code));
    assert_eq!(app.input.error.as_deref(), Some("No room has that code."));
}

#[test]
fn quick_play_on_a_small_terminal_warns_first() {
    let mut app = online();
    app.resize(Size::new(60, 20));
    assert_eq!(choose(&mut app, HomeItem::QuickPlay), []);
    assert!(app.online.notice.is_some());
    assert_eq!(app.handle(Key::Enter), [quick_play()]);
    assert_eq!(app.screen, Screen::Finding);
    app.tick(Duration::from_secs(2));
    assert_eq!(app.online.waited, Duration::from_secs(2));
    assert_eq!(app.handle(Key::Esc), [Effect::LeaveQueue]);
}

fn matched_view() -> Arc<RoomView> {
    let settings = RoomSettings::new(quick_rules(), RoomKind::Match).unwrap();
    let players = [1, 2]
        .map(|id| Entrant {
            id,
            name: format!("~p{id}"),
        })
        .to_vec();
    Arc::new(Room::new_match(settings, 1, players).view())
}

#[test]
fn the_host_adds_bots_and_takes_them_out() {
    let settings = RoomSettings::new(Rules::default(), RoomKind::Private).unwrap();
    let mut round = Room::new(settings, 1);
    for id in 1..=2 {
        let name = format!("~p{id}");
        round.join(Entrant { id, name }).unwrap();
    }
    let mut host = online();
    host.enter_room(1, Arc::new(round.view()));
    assert_eq!(host.handle(Key::Char('b')), [Effect::AddBot]);
    assert_eq!(host.handle(Key::Char('x')), [], "no bot to take out");
    assert!(round.add_bot(1));
    host.update_room(Arc::new(round.view()));
    assert_eq!(host.handle(Key::Char('x')), [Effect::RemoveBot]);
    while round.add_bot(1) {}
    host.update_room(Arc::new(round.view()));
    assert_eq!(host.handle(Key::Char('b')), []);
    assert_eq!(notice(&host), Some("The room is full."));

    let mut guest = online();
    guest.enter_room(2, Arc::new(round.view()));
    assert_eq!(guest.handle(Key::Char('b')), [], "only the host");
    assert_eq!(guest.handle(Key::Char('x')), []);
}

#[test]
fn quick_play_offers_bots_after_a_while() {
    let mut app = online();
    assert_eq!(choose(&mut app, HomeItem::QuickPlay), [quick_play()]);
    assert_eq!(app.handle(Key::Char('b')), [], "people may come yet");
    app.online.waited = BOTS_OFFERED_AFTER;
    let effects = app.handle(Key::Char('b'));
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::PlayBots {
                game: GameId::Snake,
                ..
            }]
        ),
        "{effects:?}"
    );
    assert_eq!(app.screen, Screen::Finding, "until the match comes");
    app.enter_room(1, matched_view());
    assert_eq!(app.screen, Screen::Room);
}

#[test]
fn leaving_a_match_asks_first_while_the_snake_is_alive() {
    let mut app = online();
    app.screen = Screen::Finding;
    app.enter_room(1, matched_view());
    assert_eq!(app.screen, Screen::Room);
    assert_eq!(app.handle(Key::Up), [steer(Direction::Up)]);
    assert_eq!(app.handle(Key::Esc), []);
    assert!(app.online.room.as_ref().unwrap().confirm_leave);
    assert_eq!(
        app.handle(Key::Left),
        [steer(Direction::Left)],
        "still steering"
    );
    assert_eq!(app.handle(Key::Char('n')), []);
    app.handle(Key::Esc);
    assert_eq!(app.handle(Key::Char('y')), [Effect::LeaveRoom]);
    assert_eq!(app.screen, Screen::Home { selected: 1 });
}

/// A private room's lobby with players 1 and 2, at `tick_ms`.
fn lobby(tick_ms: u64) -> Arc<RoomView> {
    let rules = Rules {
        tick_ms,
        ..Default::default()
    };
    let settings = RoomSettings::new(rules, RoomKind::Private).unwrap();
    let mut round = Room::new(settings, 1);
    for id in 1..=2 {
        let name = format!("~p{id}");
        round.join(Entrant { id, name }).unwrap();
    }
    Arc::new(round.view())
}

#[test]
fn the_host_changes_the_speed_a_step_at_a_time() {
    let speed = |tick_ms| {
        let rules = Rules {
            tick_ms,
            ..lobby(150).settings.clone()
        };
        [set_rules(rules)]
    };
    let mut host = online();
    host.enter_room(1, lobby(150));
    assert_eq!(host.handle(Key::Right), speed(175));
    // The room decides; until its next view, the speed here stays.
    assert_eq!(host.handle(Key::Char('h')), speed(125));
    host.update_room(lobby(160));
    assert_eq!(
        host.handle(Key::Left),
        speed(150),
        "back onto the settings' grid"
    );
    host.update_room(lobby(TICK_MS_MIN));
    assert_eq!(host.handle(Key::Left), [], "already the fastest");
    host.update_room(lobby(TICK_MS_MAX));
    assert_eq!(host.handle(Key::Right), [], "already the slowest");

    let mut guest = online();
    guest.enter_room(2, lobby(150));
    assert_eq!(guest.handle(Key::Right), [], "only the host");
}

fn notice(app: &App) -> Option<&str> {
    app.online.notice.as_deref()
}

#[test]
fn everyone_reads_the_rules_and_only_the_host_changes_them() {
    let mut host = online();
    host.enter_room(1, lobby(150));
    assert_eq!(host.handle(Key::Char('e')), []);
    assert_eq!(host.screen, Screen::Game);
    assert_eq!(snake(&host).screen, SnakeScreen::RoomRules { selected: 0 });
    // Board width, height, tick speed, then edge walls.
    for _ in 0..3 {
        host.handle(Key::Down);
    }
    let open = Rules {
        wall_mode: snake_core::config::WallMode::Open,
        ..lobby(150).settings.clone()
    };
    assert_eq!(host.handle(Key::Right), [set_rules(open.clone())]);
    let mut changed = (*lobby(150)).clone();
    changed.settings = open;
    host.update_room(Arc::new(changed.clone()));
    assert_eq!(notice(&host), None, "the host knows what they did");
    assert_eq!(host.handle(Key::Esc), []);
    assert_eq!(host.screen, Screen::Room);

    let mut guest = online();
    guest.enter_room(2, lobby(150));
    guest.handle(Key::Char('e'));
    assert_eq!(guest.handle(Key::Right), [], "only the host");
    guest.handle(Key::Down);
    assert_eq!(snake(&guest).screen, SnakeScreen::RoomRules { selected: 1 });
    guest.update_room(Arc::new(changed.clone()));
    assert_eq!(notice(&guest), Some("~p1 set edge walls to none (wrap)."));
    changed.settings.width = 40;
    changed.settings.poison_interval_s = 0;
    guest.update_room(Arc::new(changed.clone()));
    assert_eq!(notice(&guest), Some("~p1 changed the rules."));

    // The round starting closes the rules.
    changed.shell.phase = PhaseView::Countdown {
        left: Duration::from_secs(3),
    };
    guest.update_room(Arc::new(changed));
    assert_eq!(guest.screen, Screen::Room);
}

#[test]
fn the_host_cant_shrink_the_board_under_the_players() {
    let rules = Rules {
        width: 20,
        height: 12,
        ..Rules::default()
    };
    let mut round = Room::new(RoomSettings::new(rules, RoomKind::Private).unwrap(), 1);
    for id in 1..=4 {
        let name = format!("~p{id}");
        round.join(Entrant { id, name }).unwrap();
    }
    let mut host = online();
    host.enter_room(1, Arc::new(round.view()));
    host.handle(Key::Char('e'));
    // Narrower and narrower, as the room allows.
    for _ in 0..20 {
        match &host.handle(Key::Left)[..] {
            [Effect::Game(GameId::Snake, GameFx::Settings(bytes))] => {
                let rules: Rules = decode(bytes).unwrap();
                assert!(round.set_settings(1, rules));
                host.update_room(Arc::new(round.view()));
            }
            [] => break,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(notice(&host), Some("4 snakes don't fit on that board."));
    let narrower = Rules {
        width: round.view().settings.width - 1,
        ..round.view().settings
    };
    assert!(max_players_for(&narrower).is_none_or(|n| n < 4));
    assert_eq!(round.view().shell.max_players, 4);
}

#[test]
fn a_closed_room_sends_the_player_home_with_the_reason() {
    let mut app = online();
    app.enter_room(1, matched_view());
    let mut closed = (*matched_view()).clone();
    closed.shell.phase = PhaseView::Closed {
        reason: "The match is over.".into(),
    };
    app.update_room(Arc::new(closed));
    assert!(!app.in_room());
    assert_eq!(app.screen, Screen::Home { selected: 1 });
    assert_eq!(notice(&app), Some("The match is over."));
}

#[test]
fn guests_can_sign_up_and_accounts_see_friends() {
    let guest = online();
    let items = guest.home_items();
    assert!(items.contains(&HomeItem::SignUp) && !items.contains(&HomeItem::Friends));
    let account = signed_in();
    let items = account.home_items();
    assert!(items.contains(&HomeItem::Friends) && !items.contains(&HomeItem::SignIn));
    assert_eq!(account.online.name, "alice", "no ~ for accounts");
}

#[test]
fn the_leaderboard_sits_above_quit_and_loads_fresh_each_time() {
    for mut app in [online(), signed_in()] {
        let items = app.home_items();
        assert_eq!(
            items[items.len() - 2..],
            [HomeItem::Leaderboard, HomeItem::Quit]
        );
        assert_eq!(
            choose(&mut app, HomeItem::Leaderboard),
            [Effect::LoadLeaderboard(QUICK_RATING)]
        );
        assert_eq!(app.screen, Screen::Leaderboard { category: 0 });
        app.set_leaderboard(LeaderboardView {
            key: QUICK_RATING,
            rows: Vec::new(),
            me: MyPlace::Guest,
        });
        assert!(app.online.leaderboard.is_some());
        assert_eq!(app.handle(Key::Esc), []);
        let at = items.len() - 2;
        assert_eq!(app.screen, Screen::Home { selected: at });

        choose(&mut app, HomeItem::Leaderboard);
        assert_eq!(
            app.online.leaderboard, None,
            "the old list is gone until the new one loads"
        );
        assert_eq!(app.handle(Key::Char('x')), []);
        assert_eq!(app.screen, Screen::Leaderboard { category: 0 });
        app.handle(Key::Enter);
        assert_eq!(app.screen, Screen::Home { selected: at });
    }
}

#[test]
fn sign_up_checks_the_passwords_match_and_never_shows_them() {
    let mut app = online();
    choose(&mut app, HomeItem::SignUp);
    assert_eq!(app.screen, Screen::Form(FormKind::SignUp));
    type_in(&mut app, "Bob 1!");
    assert_eq!(app.form.username, "Bob1", "only name characters");
    app.handle(Key::Enter);
    type_in(&mut app, "correct horse");
    app.handle(Key::Enter);
    type_in(&mut app, "correct hors");
    assert_eq!(app.handle(Key::Enter), []);
    assert_eq!(
        app.form.error.as_deref(),
        Some("The passwords don't match.")
    );
    assert!(app.form.password.is_empty(), "cleared after a problem");
    type_in(&mut app, "correct horse");
    app.handle(Key::Enter);
    type_in(&mut app, "correct horse");
    let effects = app.handle(Key::Enter);
    let [Effect::SignUp { username, password }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(
        (username.as_str(), password.expose()),
        ("Bob1", "correct horse")
    );
    assert!(!format!("{effects:?}").contains("horse"), "Debug hides it");
    assert!(app.form.busy);
    assert_eq!(app.handle(Key::Char('x')), [], "waiting for the answer");
    app.form_failed("That username is taken.".into());
    assert!(!app.form.busy);
}

#[test]
fn sign_in_offers_to_remember_only_a_machine_with_a_key() {
    assert_eq!(
        FormKind::SignIn.rows(false),
        [FormRow::Username, FormRow::Password]
    );
    let mut app = online();
    choose(&mut app, HomeItem::SignIn);
    type_in(&mut app, "alice");
    app.handle(Key::Enter);
    type_in(&mut app, "secret pw");
    app.handle(Key::Down);
    app.handle(Key::Char(' '));
    let effects = app.handle(Key::Enter);
    assert!(matches!(
        effects.as_slice(),
        [Effect::SignIn {
            remember: false,
            ..
        }]
    ));
    app.signed_in(Profile {
        username: "alice".into(),
        ratings: vec![snake_rating(1040, 3, 1)],
    });
    assert!(app.home_items().contains(&HomeItem::Account));
}

#[test]
fn friends_in_a_game_can_be_watched_and_left() {
    let mut app = signed_in();
    choose(&mut app, HomeItem::Friends);
    let friend = |name: &str, watch| FriendRow {
        name: name.into(),
        status: "in a match".into(),
        online: true,
        watch,
    };
    app.set_friends(FriendsView {
        friends: vec![friend("bob", true), friend("cat", false)],
        ..FriendsView::default()
    });
    // Add, then bob, then cat.
    app.handle(Key::Down);
    assert_eq!(app.handle(Key::Char('v')), [Effect::Watch("bob".into())]);
    assert_eq!(app.screen, Screen::JoiningToWatch);
    app.watch_failed("That game is over.".into());
    assert_eq!(app.screen, Screen::Friends { selected: 0 });
    assert_eq!(notice(&app), Some("That game is over."));
    app.handle(Key::Down);
    app.handle(Key::Down);
    assert_eq!(app.handle(Key::Char('v')), []);
    assert_eq!(notice(&app), Some("cat isn't playing a game to watch."));

    // Watching bob, in a match without a seat: no keys play.
    app.screen = Screen::JoiningToWatch;
    let mut view = (*matched_view()).clone();
    view.shell.seats[0].who.name = "bob".into();
    app.watch_room(9, "bob".into(), Arc::new(view));
    assert_eq!(app.screen, Screen::Room);
    let room = app.online.room.as_ref().unwrap();
    assert_eq!(room.followed().map(|s| s.who.id), Some(1), "bob's seat");
    for key in [Key::Up, Key::Enter, Key::Char('r')] {
        assert_eq!(app.handle(key), []);
    }
    assert_eq!(
        app.handle(Key::Esc),
        [Effect::LeaveRoom, Effect::LoadFriends]
    );
    assert_eq!(app.screen, Screen::Friends { selected: 0 });
    assert!(!app.in_room());

    // A game that ends sends the watcher home, saying so.
    app.screen = Screen::JoiningToWatch;
    app.watch_room(9, "bob".into(), matched_view());
    app.room_closed(None);
    assert_eq!(notice(&app), Some("That game is over."));
}

#[test]
fn friends_are_invited_removed_on_a_second_x_and_requests_answered() {
    let mut app = signed_in();
    assert_eq!(choose(&mut app, HomeItem::Friends), [Effect::LoadFriends]);
    app.set_friends(FriendsView {
        friends: vec![
            FriendRow {
                name: "bob".into(),
                status: "online".into(),
                online: true,
                watch: false,
            },
            FriendRow {
                name: "cat".into(),
                status: "last seen today".into(),
                online: false,
                watch: false,
            },
        ],
        incoming: vec!["dan".into()],
        outgoing: vec!["eve".into()],
    });
    assert_eq!(app.online.requests, 1);
    // Add, dan's request, bob, cat, eve's.
    app.handle(Key::Down);
    assert_eq!(
        app.handle(Key::Char('a')),
        [Effect::AnswerFriend {
            name: "dan".into(),
            accept: true
        }]
    );
    app.handle(Key::Down);
    app.handle(Key::Down);
    assert_eq!(app.handle(Key::Enter), [], "cat is offline");
    app.handle(Key::Up);
    assert_eq!(app.handle(Key::Char('x')), []);
    assert_eq!(
        app.handle(Key::Char('x')),
        [Effect::RemoveFriend("bob".into())]
    );
    assert_eq!(app.handle(Key::Enter), [Effect::Invite("bob".into())]);
    assert_eq!(app.screen, Screen::Joining { by_code: false });
}

#[test]
fn invites_wait_during_a_match_then_enter_accepts() {
    let mut app = signed_in();
    app.screen = Screen::Finding;
    app.enter_room(1, matched_view());
    app.invited("bob".into(), GameId::Snake);
    assert!(!app.invite_showing(), "waits while the snake is in play");
    assert_eq!(app.handle(Key::Up), [steer(Direction::Up)]);
    app.go_home(HomeItem::QuickPlay, None);
    app.online.room = None;
    assert!(app.invite_showing());
    assert_eq!(app.handle(Key::Enter), [Effect::AcceptInvite]);
    assert_eq!(app.screen, Screen::Joining { by_code: false });

    app.invited("cat".into(), GameId::Snake);
    app.screen = Screen::Finding;
    assert_eq!(app.handle(Key::Esc), [Effect::DismissInvite]);
    assert_eq!(app.screen, Screen::Finding, "Esc went to the invite");
}
