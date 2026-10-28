//! The app with two toy games: picking one, switching with `g`, and rooms
//! that open their own game.

use std::sync::Arc;
use std::time::Duration;

use game_api::{
    AnyView, Entrant, Game, GameId, RatingKey, Room, RoomKind, RoomRng, RoomSettings, RoundEnd,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Size;
use ratatui::style::Color;

use crate::online::{Category, HomeItem, InRoom, LeaderboardView, MyPlace, Profile, ShownRating};
use crate::{App, Ctx, Effect, GameUi, Key, QuickPlay, Screen, ui};

/// A game with nothing to it, played as Snake or as Chess, so the app
/// has two to choose from.
struct Toy<const CHESS: bool>;

struct ToyRound;

impl<const CHESS: bool> Game for Toy<CHESS> {
    const ID: GameId = if CHESS { GameId::Chess } else { GameId::Snake };
    type Settings = ();
    type Input = ();
    type View = ();
    type Round = ToyRound;

    fn seats(_: &()) -> Option<usize> {
        Some(2)
    }
}

impl<const CHESS: bool> game_api::Round<Toy<CHESS>> for ToyRound {
    fn start(_: &(), _: &[Entrant], _: &mut RoomRng) -> Option<Self> {
        Some(Self)
    }
    fn go(&mut self, _: Duration) -> Option<RoundEnd> {
        None
    }
    fn advance(&mut self, _: Duration, _: Duration) -> Option<RoundEnd> {
        None
    }
    fn input(&mut self, _: u8, _: (), _: Duration, _: Option<Duration>) -> Option<RoundEnd> {
        None
    }
    fn leave(&mut self, _: u8, _: Duration) -> Option<RoundEnd> {
        None
    }
    fn until_next(&self, _: Duration) -> Duration {
        Duration::from_secs(1)
    }
    fn view(&self, _: Duration) {}
    fn take_changed(&mut self) -> bool {
        false
    }
}

/// A toy game's screens: one entry of its own, which opens nothing much.
struct ToyUi {
    id: GameId,
    title: &'static str,
}

impl GameUi for ToyUi {
    fn id(&self) -> GameId {
        self.id
    }

    fn title(&self) -> &'static str {
        self.title
    }

    fn logo(&self) -> (&'static [&'static str], Color) {
        (&["TOY"], Color::Cyan)
    }

    fn home_items(&self) -> &'static [&'static str] {
        &["Practice"]
    }

    fn room_settings(&self) -> Vec<u8> {
        Vec::new()
    }

    fn quick_play(&self, _: Size) -> QuickPlay {
        QuickPlay {
            pool: "duel",
            too_small: None,
        }
    }

    fn ratings(&self) -> &'static [Category] {
        &[
            Category {
                id: "duel",
                label: "Duels",
            },
            Category {
                id: "brawl",
                label: "Brawls",
            },
        ]
    }

    fn draw_round(&self, _: &mut Frame, _: &InRoom, _: &Ctx) {}
}

fn toys() -> Vec<Box<dyn GameUi>> {
    vec![
        Box::new(ToyUi {
            id: GameId::Snake,
            title: "Snake",
        }),
        Box::new(ToyUi {
            id: GameId::Chess,
            title: "Chess",
        }),
    ]
}

fn app(start: Option<GameId>) -> App {
    let mut app = App::online(toys(), start, "~ann".into(), true);
    app.resize(Size::new(80, 24));
    app
}

/// A private room of `G` with player 1 seated.
fn room_of<G: Game<Settings = ()>>() -> Arc<dyn AnyView> {
    let mut room = Room::<G>::new(RoomSettings::new((), RoomKind::Private).unwrap(), 1);
    room.join(Entrant {
        id: 1,
        name: "~ann".into(),
    })
    .unwrap();
    Arc::new(room.view())
}

fn shown(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..24)
        .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_new_player_picks_a_game_and_can_leave_from_there() {
    let mut app = app(None);
    assert_eq!(app.screen, Screen::Picker { selected: 0 });
    assert_eq!(app.game(), None, "none open yet");
    let picker = shown(&app);
    assert!(picker.contains("Pick a game"), "{picker}");
    assert!(
        picker.contains("Snake") && picker.contains("Chess"),
        "{picker}"
    );
    assert!(picker.contains("q quit"), "{picker}");
    app.handle(Key::Down);
    assert_eq!(app.handle(Key::Enter), []);
    assert_eq!(app.game(), Some(GameId::Chess));
    assert_eq!(app.screen, Screen::Home { selected: 0 });

    let mut leaving = self::app(None);
    assert_eq!(leaving.handle(Key::Char('q')), [Effect::Quit]);
}

#[test]
fn the_game_asked_for_opens_at_once() {
    let app = app(Some(GameId::Chess));
    assert_eq!(app.screen, Screen::Home { selected: 0 });
    assert_eq!(app.game(), Some(GameId::Chess));
    assert!(shown(&app).contains("g games"));
}

#[test]
fn a_last_game_no_longer_served_opens_the_only_game_there_is() {
    let mut chess = toys();
    chess.remove(0);
    let app = App::online(chess, Some(GameId::Snake), "~ann".into(), true);
    assert_eq!(app.game(), Some(GameId::Chess));
    assert_eq!(app.screen, Screen::Home { selected: 0 });
}

#[test]
fn with_one_game_there_is_nothing_to_pick() {
    let mut one = toys();
    one.truncate(1);
    let mut app = App::online(one, None, "~ann".into(), true);
    assert_eq!(app.screen, Screen::Home { selected: 0 });
    assert_eq!(app.game(), Some(GameId::Snake));
    assert!(!app.home_items().contains(&HomeItem::Games));
    assert_eq!(app.handle(Key::Char('g')), []);
    assert_eq!(app.screen, Screen::Home { selected: 0 });
    assert!(!shown(&app).contains("g games"));
}

#[test]
fn g_switches_games_and_esc_goes_back() {
    let mut app = app(Some(GameId::Snake));
    app.handle(Key::Char('G'));
    assert_eq!(
        app.screen,
        Screen::Picker { selected: 0 },
        "at the open game"
    );
    assert!(shown(&app).contains("playing now"));
    app.handle(Key::Esc);
    let games = app.home_items().iter().position(|i| *i == HomeItem::Games);
    assert_eq!(
        app.screen,
        Screen::Home {
            selected: games.unwrap()
        }
    );
    assert_eq!(app.game(), Some(GameId::Snake), "unchanged");

    app.handle(Key::Char('g'));
    app.handle(Key::Up);
    app.handle(Key::Enter);
    assert_eq!(app.game(), Some(GameId::Chess));
    // Quick play and the leaderboard are the open game's.
    app.screen = Screen::Home { selected: 1 };
    assert_eq!(
        app.handle(Key::Enter),
        [Effect::QuickPlay {
            game: GameId::Chess,
            pool: "duel"
        }]
    );
}

#[test]
fn other_games_sits_above_the_leaderboard() {
    let app = app(Some(GameId::Snake));
    let items = app.home_items();
    assert_eq!(
        items[items.len() - 3..],
        [HomeItem::Games, HomeItem::Leaderboard, HomeItem::Quit]
    );
    assert_eq!(items[0], HomeItem::Game(0));
    assert_eq!(app.label(items[0]), "Practice");
}

#[test]
fn a_room_opens_its_own_game() {
    let mut app = app(Some(GameId::Snake));
    app.screen = Screen::Joining { by_code: true };
    assert!(app.enter_room(1, room_of::<Toy<true>>()));
    assert_eq!(app.game(), Some(GameId::Chess));
    assert_eq!(app.screen, Screen::Room);
    // Views of another game's room are someone else's mistake.
    app.update_room(room_of::<Toy<false>>());
    assert_eq!(app.online.room.as_ref().unwrap().view.game(), GameId::Chess);
    // Leaving lands on that game's home.
    app.handle(Key::Esc);
    assert_eq!(app.game(), Some(GameId::Chess));
    assert!(matches!(app.screen, Screen::Home { .. }));
}

#[test]
fn rooms_of_games_the_app_lacks_are_turned_down() {
    let mut one = toys();
    one.truncate(1);
    let mut app = App::online(one, None, "~ann".into(), true);
    app.screen = Screen::Joining { by_code: true };
    assert!(!app.enter_room(1, room_of::<Toy<true>>()));
    assert!(!app.in_room());
    assert!(!app.watch_room(1, "bob".into(), room_of::<Toy<true>>()));
}

#[test]
fn invites_to_another_game_say_which() {
    let mut app = app(Some(GameId::Snake));
    app.invited("bob".into(), GameId::Chess);
    let banner = shown(&app);
    assert!(banner.contains("bob invites you to play Chess"), "{banner}");
    app.invited("bob".into(), GameId::Snake);
    let banner = shown(&app);
    assert!(banner.contains("bob invites you to play ·"), "{banner}");
}

#[test]
fn the_picker_and_games_ratings_draw_at_every_size() {
    let apps = [app(None), app(Some(GameId::Chess))].map(|mut app| {
        app.screen = Screen::Picker { selected: 1 };
        app
    });
    let mut account = app(Some(GameId::Snake));
    sign_in(&mut account);
    account.screen = Screen::Account { selected: 0 };
    let mut board_shown = app(Some(GameId::Snake));
    board_shown.screen = Screen::Leaderboard { category: 1 };
    board_shown.set_leaderboard(board(key(GameId::Snake, "brawl")));
    let apps: Vec<App> = apps.into_iter().chain([account, board_shown]).collect();
    for w in 1..=90 {
        for h in 1..=30 {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            for app in &apps {
                terminal.draw(|frame| ui::draw(frame, app)).unwrap();
            }
        }
    }
}

#[test]
fn bots_are_offered_only_by_games_with_bots() {
    let mut app = app(Some(GameId::Snake));
    app.screen = Screen::Finding;
    app.online.waited = crate::app::BOTS_OFFERED_AFTER;
    assert!(!app.bots_offered(), "toys have no bots");
    assert_eq!(app.handle(Key::Char('b')), []);
    assert!(!shown(&app).contains("bots"));
}

#[test]
fn hosts_of_games_without_bots_add_none() {
    let mut app = app(Some(GameId::Snake));
    assert!(app.enter_room(1, room_of::<Toy<false>>()));
    assert_eq!(app.handle(Key::Char('b')), []);
    let lobby = shown(&app);
    assert!(!lobby.contains("add a bot"), "{lobby}");
    assert!(lobby.contains("Enter ready · Esc leave"), "{lobby}");
}

fn key(game: GameId, category: &'static str) -> RatingKey {
    RatingKey { game, category }
}

/// Signed in as ann, with a rating in each of both games' categories.
fn sign_in(app: &mut App) {
    let shown = |key, rating, games, wins| ShownRating {
        key,
        rating,
        games,
        wins,
    };
    app.set_profile(Some(Profile {
        username: "ann".into(),
        ratings: vec![
            shown(key(GameId::Snake, "duel"), 1100, 3, 2),
            shown(key(GameId::Snake, "brawl"), 900, 1, 0),
            shown(key(GameId::Chess, "duel"), 1500, 0, 0),
            shown(key(GameId::Chess, "brawl"), 1500, 0, 0),
        ],
    }));
}

fn board(key: RatingKey) -> LeaderboardView {
    LeaderboardView {
        key,
        rows: Vec::new(),
        me: MyPlace::Guest,
    }
}

#[test]
fn each_rating_has_its_own_leaderboard() {
    let mut app = app(Some(GameId::Snake));
    let at = app
        .home_items()
        .iter()
        .position(|i| *i == HomeItem::Leaderboard);
    app.screen = Screen::Home {
        selected: at.unwrap(),
    };
    let duels = key(GameId::Snake, "duel");
    let brawls = key(GameId::Snake, "brawl");
    assert_eq!(app.handle(Key::Enter), [Effect::LoadLeaderboard(duels)]);
    assert_eq!(app.screen, Screen::Leaderboard { category: 0 });
    assert!(shown(&app).contains("Leaderboard · Snake · Duels"));

    assert_eq!(app.handle(Key::Right), [Effect::LoadLeaderboard(brawls)]);
    assert_eq!(app.screen, Screen::Leaderboard { category: 1 });
    // The duels' board, answering too late, is dropped.
    app.set_leaderboard(board(duels));
    assert_eq!(app.online.leaderboard, None);
    app.set_leaderboard(board(brawls));
    assert!(app.online.leaderboard.is_some());
    let brawl = shown(&app);
    assert!(brawl.contains("Brawls is rated."), "{brawl}");
    assert!(brawl.contains("←→ rating · Esc back"), "{brawl}");

    assert_eq!(app.handle(Key::Char('d')), [Effect::LoadLeaderboard(duels)]);
    assert_eq!(app.online.leaderboard, None, "loads afresh");
    assert_eq!(app.handle(Key::Left), [Effect::LoadLeaderboard(brawls)]);
    app.handle(Key::Esc);
    assert!(matches!(app.screen, Screen::Home { .. }));
}

#[test]
fn the_account_shows_the_open_games_ratings() {
    let mut app = app(Some(GameId::Snake));
    sign_in(&mut app);
    let home = shown(&app);
    assert!(home.contains("Signed in as ann · rating 1100"), "{home}");
    app.screen = Screen::Account { selected: 0 };
    let account = shown(&app);
    assert!(
        account.contains("Rating 1100 · 3 games · 2 wins in duels"),
        "{account}"
    );
    assert!(
        account.contains("Rating 900 · 1 games · 0 wins in brawls"),
        "{account}"
    );

    // A rated round moves only its own rating.
    app.rated(key(GameId::Snake, "brawl"), 950, true);
    let profile = app.online.account.as_ref().unwrap();
    let brawl = profile.rating(key(GameId::Snake, "brawl")).unwrap();
    assert_eq!((brawl.rating, brawl.games, brawl.wins), (950, 2, 1));
    let duel = profile.rating(key(GameId::Snake, "duel")).unwrap();
    assert_eq!((duel.rating, duel.games), (1100, 3));

    // Another game shows its own.
    app.screen = Screen::Home { selected: 0 };
    app.handle(Key::Char('g'));
    app.handle(Key::Down);
    app.handle(Key::Enter);
    assert_eq!(app.game(), Some(GameId::Chess));
    assert!(shown(&app).contains("rating 1500"));
}
