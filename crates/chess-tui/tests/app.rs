//! Chess in the app: the keys that play a game, a room's challenge, and
//! the settings.

mod common;

use chess_core::control::TimeControl;
use chess_core::moves::Move;
use chess_core::round::{ChessInput, ChessSettings, Side};
use chess_core::square::Square;
use chess_tui::{BoardLook, ChessScreen, Hit, MoveInput, Prefs};
use common::*;
use game_api::codec::decode;
use game_api::{GameId, RatingKey};
use shell_tui::online::{HomeItem, Profile, ShownRating};
use shell_tui::{App, Effect, GameFx, Key, Mouse, MouseKind, Screen};

/// The moves and other inputs the app sent to the room.
fn inputs(effects: &[Effect]) -> Vec<ChessInput> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Game(GameId::Chess, GameFx::Play(bytes)) => decode(bytes),
            _ => None,
        })
        .collect()
}

fn uci(text: &str) -> ChessInput {
    ChessInput::Move(Move::parse_uci(text).unwrap())
}

fn keys(app: &mut App, keys: &[Key]) -> Vec<Effect> {
    keys.iter().flat_map(|&k| app.handle(k)).collect()
}

#[test]
fn typed_moves_are_read_as_typed() {
    let mut app = app_in(ANN, playing(&[]).view());
    assert_eq!(inputs(&typed(&mut app, "e4\r")), [uci("e2e4")]);
    // Case matters to SAN: B is a bishop, b a pawn.
    let mut app = app_in(ANN, playing(&["e2e4", "e7e5"]).view());
    assert_eq!(inputs(&typed(&mut app, "Nf3\r")), [uci("g1f3")]);
    let mut app = app_in(ANN, playing(&["e2e4", "e7e5"]).view());
    assert_eq!(inputs(&typed(&mut app, "Bc4\r")), [uci("f1c4")]);
    // Enough to tell, and Backspace takes a letter back.
    let mut app = app_in(ANN, playing(&["e2e4", "e7e5"]).view());
    assert_eq!(inputs(&typed(&mut app, "Qx")), []);
    app.handle(Key::Backspace);
    app.handle(Key::Backspace);
    assert_eq!(inputs(&typed(&mut app, "g1f\r")), [uci("g1f3")]);
}

#[test]
fn only_your_own_moves_go_and_only_in_turn() {
    let mut app = app_in(BEN, playing(&[]).view());
    assert_eq!(inputs(&typed(&mut app, "e5\r")), [], "white moves first");
    let screen = shown(&app, 80, 24);
    assert!(screen.contains("Premove e5"), "kept for his turn: {screen}");
    let mut app = app_in(BEN, playing(&[]).view());
    chess(&mut app).prefs.premoves = false;
    typed(&mut app, "e5\r");
    let screen = shown(&app, 80, 24);
    assert!(screen.contains("Wait for your move."), "{screen}");
    let mut app = app_in(ANN, playing(&[]).view());
    assert_eq!(inputs(&typed(&mut app, "e5\r")), [], "not a white move");
    let screen = shown(&app, 80, 24);
    assert!(screen.contains("isn't a legal move"), "{screen}");
}

#[test]
fn arrows_pick_up_a_piece_and_put_it_down() {
    let mut app = app_in(ANN, playing(&[]).view());
    // The first arrow shows the cursor on the king's pawn; up is up the
    // board.
    let effects = keys(
        &mut app,
        &[Key::Up, Key::Enter, Key::Up, Key::Up, Key::Enter],
    );
    assert_eq!(inputs(&effects), [uci("e2e4")]);
    // Black sees the board the other way round.
    let mut app = app_in(BEN, playing(&["d2d4"]).view());
    let effects = keys(
        &mut app,
        &[
            Key::Up,
            Key::Right,
            Key::Enter,
            Key::Up,
            Key::Up,
            Key::Enter,
        ],
    );
    assert_eq!(inputs(&effects), [uci("d7d5")]);
}

#[test]
fn cursor_keys_move_instead_of_typing() {
    let mut app = app_in(ANN, playing(&[]).view());
    chess(&mut app).prefs.input = MoveInput::Cursor;
    app.handle(Key::Up);
    let effects = typed(&mut app, " ww ");
    assert_eq!(inputs(&effects), [uci("e2e4")]);
}

#[test]
fn the_menu_aborts_resigns_and_offers_draws() {
    let mut app = app_in(ANN, playing(&[]).view());
    app.handle(Key::Esc);
    let screen = shown(&app, 80, 24);
    assert!(screen.contains("Abort the game"), "{screen}");
    assert!(
        !screen.contains("Resign"),
        "nothing to resign yet: {screen}"
    );
    assert_eq!(inputs(&app.handle(Key::Enter)), [ChessInput::Abort]);

    let mut app = app_in(ANN, playing(&["e2e4", "e7e5"]).view());
    app.handle(Key::Esc);
    assert_eq!(inputs(&app.handle(Key::Enter)), [ChessInput::OfferDraw]);
    // Resigning asks first.
    app.handle(Key::Esc);
    keys(&mut app, &[Key::Down, Key::Enter]);
    assert!(shown(&app, 80, 24).contains("Resign this game?"));
    assert_eq!(inputs(&typed(&mut app, "n")), []);
    app.handle(Key::Esc);
    keys(&mut app, &[Key::Down, Key::Enter]);
    assert_eq!(inputs(&typed(&mut app, "y")), [ChessInput::Resign]);
}

#[test]
fn a_draw_offer_is_answered_from_the_menu() {
    let mut room = playing(&["e2e4", "e7e5"]);
    room.play(ANN, ChessInput::OfferDraw, None);
    let mut app = app_in(BEN, room.view());
    let screen = shown(&app, 80, 24);
    assert!(screen.contains("offers a draw"), "{screen}");
    app.handle(Key::Esc);
    assert_eq!(inputs(&app.handle(Key::Enter)), [ChessInput::AcceptDraw]);
}

#[test]
fn pawns_reaching_the_end_ask_what_to_become() {
    let mut view = playing(&[]).view();
    let game = view.game.as_mut().unwrap();
    game.start = Some("8/4P3/8/8/8/8/k7/4K3 w - - 0 1".to_string());
    let mut app = app_in(ANN, view.clone());
    typed(&mut app, "e8\r");
    assert!(shown(&app, 80, 24).contains("Promote to"));
    assert_eq!(inputs(&typed(&mut app, "n")), [uci("e7e8n")]);

    let mut app = app_in(ANN, view);
    chess(&mut app).prefs.auto_queen = true;
    assert_eq!(inputs(&typed(&mut app, "e8\r")), [uci("e7e8q")]);
}

/// The challenge Ann's room's next game would be.
fn challenged(effects: &[Effect]) -> Option<ChessSettings> {
    effects.iter().find_map(|e| match e {
        Effect::Game(GameId::Chess, GameFx::Settings(bytes)) => decode(bytes),
        _ => None,
    })
}

#[test]
fn the_host_sets_the_challenge() {
    let lobby = room(TimeControl::minutes(10, 0)).view();
    let mut app = app_in(ANN, lobby.clone());
    let effects = app.handle(Key::Left);
    let five_three = ChessSettings {
        control: TimeControl::minutes(5, 3),
        side: Side::White,
    };
    assert_eq!(challenged(&effects), Some(five_three));
    assert_eq!(
        chess(&mut app).prefs.challenge,
        five_three,
        "kept for next time"
    );
    // The challenge screen, for any time at all, from the 5+3 asked for
    // before the room's view shows it.
    typed(&mut app, "e");
    assert_eq!(app.screen, Screen::Game);
    let effects = keys(&mut app, &[Key::Down, Key::Right]);
    let settings = challenged(&effects).unwrap();
    assert_eq!(settings.control, TimeControl::minutes(5, 4));
    app.handle(Key::Esc);
    assert_eq!(app.screen, Screen::Room);

    // Keys faster than the room's views step on from the last one sent.
    let mut quick = app_in(ANN, lobby.clone());
    let effects = keys(&mut quick, &[Key::Left, Key::Left, Key::Left]);
    let steps: Vec<TimeControl> = effects
        .iter()
        .filter_map(|e| challenged(std::slice::from_ref(e)).map(|s| s.control))
        .collect();
    let presets = [(5, 3), (5, 0), (3, 2)].map(|(m, s)| TimeControl::minutes(m, s));
    assert_eq!(steps, presets);

    let mut ben = app_in(BEN, lobby);
    assert_eq!(challenged(&ben.handle(Key::Left)), None, "only the host");
    typed(&mut ben, "e");
    assert!(matches!(
        chess(&mut ben).screen,
        ChessScreen::Challenge { .. }
    ));
    assert!(shown(&ben, 80, 24).contains("Only ann-with-a-long-name can change these."));
}

#[test]
fn settings_are_kept() {
    let mut app = App::online(
        vec![Box::new(chess_tui::ChessUi::default())],
        None,
        "~ann".into(),
        true,
    );
    let effects = app.handle(Key::Enter);
    assert_eq!(app.screen, Screen::Game);
    assert!(effects.is_empty());
    let effects = keys(&mut app, &[Key::Right]);
    let Some(Effect::Game(GameId::Chess, GameFx::SavePrefs(text))) = effects.first() else {
        panic!("saved: {effects:?}");
    };
    assert_eq!(Prefs::from_saved_text(text).board, BoardLook::Green);
    app.handle(Key::Esc);
    assert!(matches!(app.screen, Screen::Home { .. }));
}

#[test]
fn quick_pairing_picks_a_preset_and_waits_in_its_pool() {
    let mut app = App::online(
        vec![Box::new(chess_tui::ChessUi::default())],
        None,
        "~ann".into(),
        true,
    );
    app.go_home(HomeItem::QuickPlay, None);
    let effects = app.handle(Key::Enter);
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(app.screen, Screen::Game);
    assert_eq!(
        chess(&mut app).screen,
        ChessScreen::Pairing { selected: 5 },
        "on 5+3 to begin with"
    );
    let screen = shown(&app, 80, 24);
    assert!(
        screen.contains("5+3 blitz · sides drawn · unrated"),
        "{screen}"
    );

    // Up to bullet's 2+1, left to 1+0, right again, and play it.
    let effects = keys(&mut app, &[Key::Up, Key::Left, Key::Right, Key::Enter]);
    let [
        Effect::Game(GameId::Chess, GameFx::SavePrefs(text)),
        Effect::QuickPlay {
            game: GameId::Chess,
            pool: "2+1",
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(
        Prefs::from_saved_text(text).pairing,
        TimeControl::minutes(2, 1)
    );
    assert_eq!(app.screen, Screen::Finding);
    let screen = shown(&app, 80, 24);
    assert!(screen.contains("2+1 bullet"), "{screen}");

    // Quick play opens again on the preset last played, and Esc goes
    // back to it on the home menu.
    app.handle(Key::Esc);
    app.go_home(HomeItem::QuickPlay, None);
    app.handle(Key::Enter);
    assert_eq!(chess(&mut app).screen, ChessScreen::Pairing { selected: 1 });
    app.handle(Key::Esc);
    let Screen::Home { selected } = app.screen else {
        panic!("{:?}", app.screen);
    };
    assert_eq!(app.home_items()[selected], HomeItem::QuickPlay);
}

#[test]
fn pairing_shows_the_ratings_of_a_signed_in_player() {
    let mut app = App::online(
        vec![Box::new(chess_tui::ChessUi::default())],
        None,
        "~ann".into(),
        true,
    );
    let rating = |category, rating, provisional| ShownRating {
        key: RatingKey {
            game: GameId::Chess,
            category,
        },
        rating,
        games: if provisional { 1 } else { 30 },
        wins: 1,
        provisional,
    };
    app.online.account = Some(Profile {
        username: "ann".into(),
        ratings: vec![
            rating("bullet", 1500, true),
            rating("blitz", 1712, false),
            rating("rapid", 1500, true),
            rating("classical", 1500, true),
        ],
    });
    // Home shows the rating played most.
    app.go_home(HomeItem::QuickPlay, None);
    let home = shown(&app, 80, 24);
    assert!(home.contains("Signed in as ann · blitz 1712"), "{home}");
    app.handle(Key::Enter);
    let screen = shown(&app, 80, 24);
    let blitz = screen.lines().find(|l| l.contains("Blitz")).unwrap();
    assert!(
        blitz.contains("1712") && !blitz.contains("1712?"),
        "{screen}"
    );
    let bullet = screen.lines().find(|l| l.contains("Bullet")).unwrap();
    assert!(bullet.contains("1500?"), "{screen}");
    assert!(
        screen.contains("5+3 blitz · sides drawn · rated"),
        "{screen}"
    );
}

#[test]
fn results_score_and_say_how_it_ended() {
    let mut room = playing(&["f2f3", "e7e5", "g2g4"]);
    room.play(BEN, uci("d8h4"), None);
    let app = app_in(ANN, room.view());
    let screen = shown(&app, 80, 24);
    assert!(screen.contains("ben wins!"), "{screen}");
    assert!(screen.contains("Black won by checkmate"), "{screen}");
}

/// Square `name`, as in "e2".
fn square(name: &str) -> Square {
    Square::parse(name).unwrap()
}

/// Where the screen, drawn the way the player sees it, shows `hit`.
fn point_at(app: &mut App, hit: Hit) -> (u16, u16) {
    shown(app, app.size.width, app.size.height);
    let area = chess(app)
        .hits
        .spot(hit)
        .unwrap_or_else(|| panic!("{hit:?} isn't on the screen"));
    (area.x + area.width / 2, area.y + area.height / 2)
}

/// The mouse's button going `kind` at `(x, y)`, with the screen drawn
/// first as each frame is.
fn mouse_at(app: &mut App, kind: MouseKind, (x, y): (u16, u16)) -> Vec<Effect> {
    shown(app, app.size.width, app.size.height);
    app.handle(Key::Mouse(Mouse { kind, x, y }))
}

/// The button going `kind` on `hit`.
fn mouse(app: &mut App, kind: MouseKind, hit: Hit) -> Vec<Effect> {
    let at = point_at(app, hit);
    mouse_at(app, kind, at)
}

/// A press and release on `hit`, in one place.
fn click(app: &mut App, hit: Hit) -> Vec<Effect> {
    let at = point_at(app, hit);
    let mut effects = mouse_at(app, MouseKind::Press, at);
    effects.extend(mouse_at(app, MouseKind::Release, at));
    effects
}

#[test]
fn a_click_on_a_piece_then_a_square_moves_it_and_so_does_a_drag() {
    let mut app = app_in(ANN, playing(&[]).view());
    assert!(app.wants_mouse(), "her game takes the mouse");
    assert_eq!(inputs(&click(&mut app, Hit::Square(square("e2")))), []);
    assert_eq!(chess(&mut app).picked(), Some(square("e2")));
    assert_eq!(
        inputs(&click(&mut app, Hit::Square(square("e4")))),
        [uci("e2e4")]
    );

    let mut app = app_in(ANN, playing(&[]).view());
    mouse(&mut app, MouseKind::Press, Hit::Square(square("g1")));
    let dropped = mouse(&mut app, MouseKind::Release, Hit::Square(square("f3")));
    assert_eq!(inputs(&dropped), [uci("g1f3")], "dragged");

    // Not hers, not her turn, or not a move: nothing goes.
    let mut app = app_in(ANN, playing(&[]).view());
    click(&mut app, Hit::Square(square("e7")));
    assert_eq!(chess(&mut app).picked(), None);
    click(&mut app, Hit::Square(square("e2")));
    assert_eq!(inputs(&click(&mut app, Hit::Square(square("e5")))), []);
    assert!(shown(&app, 80, 24).contains("That piece can't go there."));
    let mut app = app_in(BEN, playing(&[]).view());
    click(&mut app, Hit::Square(square("e7")));
    assert_eq!(inputs(&click(&mut app, Hit::Square(square("e5")))), []);
}

#[test]
fn the_menu_promotion_and_resigning_take_clicks() {
    let mut app = app_in(ANN, playing(&["e2e4", "e7e5"]).view());
    click(&mut app, Hit::OpenMenu);
    assert!(shown(&app, 80, 24).contains("Offer a draw"));
    // A click elsewhere closes it.
    click(&mut app, Hit::Square(square("a8")));
    assert!(!shown(&app, 80, 24).contains("Offer a draw"));
    click(&mut app, Hit::OpenMenu);
    assert_eq!(
        inputs(&click(&mut app, Hit::MenuItem(0))),
        [ChessInput::OfferDraw]
    );
    click(&mut app, Hit::OpenMenu);
    assert_eq!(inputs(&click(&mut app, Hit::MenuItem(1))), [], "asks first");
    assert!(shown(&app, 80, 24).contains("Resign this game?"));
    assert_eq!(inputs(&click(&mut app, Hit::Resign(false))), []);
    click(&mut app, Hit::OpenMenu);
    click(&mut app, Hit::MenuItem(1));
    assert_eq!(
        inputs(&click(&mut app, Hit::Resign(true))),
        [ChessInput::Resign]
    );

    let mut view = playing(&[]).view();
    let game = view.game.as_mut().unwrap();
    game.start = Some("8/4P3/8/8/8/8/k7/4K3 w - - 0 1".to_string());
    let mut app = app_in(ANN, view);
    click(&mut app, Hit::Square(square("e7")));
    click(&mut app, Hit::Square(square("e8")));
    assert!(shown(&app, 80, 24).contains("Promote to"));
    assert_eq!(inputs(&click(&mut app, Hit::Promote(3))), [uci("e7e8n")]);
}

#[test]
fn the_mouse_works_only_while_the_board_or_chess_screens_show() {
    let lobby = room(TimeControl::minutes(10, 0)).view();
    let app = app_in(ANN, lobby);
    assert!(!app.wants_mouse(), "the lobby's code can be selected");
    let mut over = playing(&["f2f3", "e7e5", "g2g4"]);
    over.play(BEN, uci("d8h4"), None);
    let app = app_in(ANN, over.view());
    assert!(!app.wants_mouse(), "the game is over");
    let app = app_in(99, playing(&[]).view());
    assert!(!app.wants_mouse(), "watching");

    let mut app = App::online(
        vec![Box::new(chess_tui::ChessUi::default())],
        None,
        "~ann".into(),
        true,
    );
    app.resize(ratatui::layout::Size::new(80, 24));
    assert!(!app.wants_mouse(), "home");
    let press = Key::Mouse(Mouse {
        kind: MouseKind::Press,
        x: 40,
        y: 12,
    });
    assert!(app.handle(press).is_empty(), "ignored at home");
    app.go_home(HomeItem::QuickPlay, None);
    app.handle(Key::Enter);
    assert!(app.wants_mouse());
    let effects = click(&mut app, Hit::Preset(0));
    assert!(
        matches!(
            effects.as_slice(),
            [_, Effect::QuickPlay { pool: "1+0", .. }]
        ),
        "{effects:?}"
    );

    app.go_home(HomeItem::Game(0), None);
    app.handle(Key::Enter);
    assert_eq!(chess(&mut app).screen, ChessScreen::Settings);
    let effects = click(&mut app, Hit::Setting(0));
    let Some(Effect::Game(GameId::Chess, GameFx::SavePrefs(text))) = effects.first() else {
        panic!("saved: {effects:?}");
    };
    assert_eq!(Prefs::from_saved_text(text).board, BoardLook::Green);
}

#[test]
fn pieces_are_pictures_where_nothing_covers_them() {
    let mut app = app_in(ANN, playing(&["e2e4", "e7e5"]).view());
    app.resize(ratatui::layout::Size::new(120, 40));
    shown(&app, 120, 40);
    assert!(
        app.pictures().is_none(),
        "until the terminal says it shows them"
    );
    app.show_pictures((8, 16));
    shown(&app, 120, 40);
    let all = app.pictures().unwrap().take();
    assert_eq!(all.len(), 32);
    assert!(
        all.iter().all(|p| (p.area.width, p.area.height) == (10, 4)),
        "the big board"
    );
    // The menu hides the pieces under it, and shows over them.
    app.handle(Key::Esc);
    let screen = shown(&app, 120, 40);
    assert!(screen.contains("Offer a draw"), "{screen}");
    let some = app.pictures().unwrap().take().len();
    assert!((16..32).contains(&some), "{some} left");
    // Where a picked-up piece can go shows as pictures too.
    app.handle(Key::Esc);
    click(&mut app, Hit::Square(square("g1")));
    shown(&app, 120, 40);
    assert_eq!(
        app.pictures().unwrap().take().len(),
        32 + 3,
        "e2, f3 and h3"
    );
    click(&mut app, Hit::Square(square("g1")));
    // Turned off, they're letters or glyphs again.
    app.handle(Key::Esc);
    chess(&mut app).prefs.images = false;
    shown(&app, 120, 40);
    assert!(app.pictures().unwrap().take().is_empty());
    let screen = shown(&app, 120, 40);
    assert!(screen.contains('♜'), "{screen}");
}

/// What the app does when the room's view becomes `room`'s.
fn moved(app: &mut App, room: game_api::Room<chess_core::round::Chess>) -> Vec<Effect> {
    app.update_room(std::sync::Arc::new(room.view()))
}

#[test]
fn a_premove_is_played_as_soon_as_the_turn_comes() {
    // Ben, black, picks e7 and e5 while white thinks.
    let mut app = app_in(BEN, playing(&[]).view());
    click(&mut app, Hit::Square(square("e7")));
    assert_eq!(inputs(&click(&mut app, Hit::Square(square("e5")))), []);
    let screen = shown(&app, 80, 24);
    assert!(screen.contains("Premove e7e5"), "{screen}");
    assert_eq!(inputs(&moved(&mut app, playing(&["e2e4"]))), [uci("e7e5")]);
    assert!(!shown(&app, 80, 24).contains("Premove"), "gone once played");

    // Typed, too, and played however the position turned out.
    let mut app = app_in(BEN, playing(&[]).view());
    assert_eq!(inputs(&typed(&mut app, "Nf6\r")), []);
    assert_eq!(inputs(&moved(&mut app, playing(&["d2d4"]))), [uci("g8f6")]);

    // Not legal by then: nothing goes, and the player hears why.
    let mut app = app_in(BEN, playing(&[]).view());
    typed(&mut app, "Qh4\r");
    assert_eq!(inputs(&moved(&mut app, playing(&["e2e4"]))), []);
    assert!(shown(&app, 80, 24).contains("Your premove wasn't legal by then."));
}

#[test]
fn a_premove_can_be_called_off_or_turned_off() {
    let mut app = app_in(BEN, playing(&[]).view());
    click(&mut app, Hit::Square(square("e7")));
    click(&mut app, Hit::Square(square("e5")));
    app.handle(Key::Backspace);
    assert_eq!(
        inputs(&moved(&mut app, playing(&["e2e4"]))),
        [],
        "called off"
    );

    // A click elsewhere calls it off too.
    let mut app = app_in(BEN, playing(&[]).view());
    click(&mut app, Hit::Square(square("e7")));
    click(&mut app, Hit::Square(square("e5")));
    click(&mut app, Hit::Square(square("a4")));
    assert_eq!(inputs(&moved(&mut app, playing(&["e2e4"]))), []);

    let mut app = app_in(BEN, playing(&[]).view());
    chess(&mut app).prefs.premoves = false;
    click(&mut app, Hit::Square(square("e7")));
    click(&mut app, Hit::Square(square("e5")));
    assert!(shown(&app, 80, 24).contains("Wait for your move."));
    assert_eq!(inputs(&moved(&mut app, playing(&["e2e4"]))), []);
}
