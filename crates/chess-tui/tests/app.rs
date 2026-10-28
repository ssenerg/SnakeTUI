//! Chess in the app: the keys that play a game, a room's challenge, and
//! the settings.

mod common;

use chess_core::control::TimeControl;
use chess_core::moves::Move;
use chess_core::round::{ChessInput, ChessSettings, Side};
use chess_tui::{BoardLook, ChessScreen, MoveInput, Prefs};
use common::*;
use game_api::GameId;
use game_api::codec::decode;
use shell_tui::online::HomeItem;
use shell_tui::{App, Effect, GameFx, Key, Screen};

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
fn settings_are_kept_and_quick_play_waits() {
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

    app.go_home(HomeItem::QuickPlay, None);
    let effects = app.handle(Key::Enter);
    assert!(effects.is_empty(), "{effects:?}");
    let screen = shown(&app, 100, 30);
    assert!(screen.contains("Make a private room"), "{screen}");
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
