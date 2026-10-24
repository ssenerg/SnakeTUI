use std::sync::Arc;
use std::time::Duration;

use ratatui::layout::Size;
use snake_core::config::{GameConfig, Rules};
use snake_core::geom::Direction;
use snake_core::records::Records;
use snake_core::round::{PhaseView, PlayerId, RoomKind, RoomView, SeatView, quick_rules};
use snake_core::settings::SETTINGS;
use snake_core::solo::SoloGame;

use crate::key::Key;
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    /// Online: the main menu.
    Home {
        selected: usize,
    },
    /// Single-player settings. The local game starts here.
    Menu,
    Playing,
    Paused {
        selected: usize,
    },
    GameOver {
        new_record: bool,
    },
    /// Online: typing into [`App::input`].
    Input(Field),
    /// Online: in the quick-play queue.
    Finding,
    /// Online: waiting to get into a private room.
    Joining {
        by_code: bool,
    },
    /// Online: in a room. What shows follows the room's phase.
    Room,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Code,
    Nickname,
}

impl Field {
    pub fn max_len(self) -> usize {
        match self {
            Field::Code => 6,
            Field::Nickname => 16,
        }
    }

    /// Only characters that are safe to show on other players' screens.
    fn accepts(self, c: char) -> bool {
        match self {
            Field::Code => c.is_ascii_alphanumeric(),
            Field::Nickname => c.is_ascii_alphanumeric() || c == '_' || c == '-',
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HomeItem {
    SinglePlayer,
    QuickPlay,
    CreateRoom,
    JoinCode,
    Nickname,
    Quit,
}

pub const HOME: [HomeItem; 6] = [
    HomeItem::SinglePlayer,
    HomeItem::QuickPlay,
    HomeItem::CreateRoom,
    HomeItem::JoinCode,
    HomeItem::Nickname,
    HomeItem::Quit,
];

impl HomeItem {
    fn index(self) -> usize {
        HOME.iter().position(|i| *i == self).unwrap()
    }

    pub fn label(self) -> &'static str {
        match self {
            HomeItem::SinglePlayer => "Single player",
            HomeItem::QuickPlay => "Quick play",
            HomeItem::CreateRoom => "Create a private room",
            HomeItem::JoinCode => "Join with a code",
            HomeItem::Nickname => "Nickname",
            HomeItem::Quit => "Quit",
        }
    }
}

/// Work the app leaves to whoever runs it, in the order given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Persist [`App::cfg`].
    SaveConfig,
    /// A new best was just set in [`App::records`]. By the time this is
    /// handled the game may be gone, so it says which record changed.
    RecordSet {
        key: String,
        best: u32,
    },
    /// Leave the app.
    Quit,
    /// Join the quick-play queue; answer with [`App::enter_room`].
    QuickPlay,
    LeaveQueue,
    /// Make a private room with these rules and join it; answer with
    /// [`App::enter_room`] or [`App::join_failed`].
    CreateRoom(Rules),
    /// Join the private room with this code.
    JoinRoom(String),
    Steer(Direction),
    ToggleReady,
    LeaveRoom,
    /// Answer with [`App::set_name`].
    SetNickname(String),
}

/// What's being typed on an input screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    pub error: Option<String>,
}

/// What an online player's app knows beyond the local game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Online {
    /// How others see this player, such as `~alice`.
    pub name: String,
    /// Shown on the home screen until the next key.
    pub notice: Option<String>,
    pub room: Option<InRoom>,
    /// Time spent in the queue so far.
    pub waited: Duration,
    pub queue_len: usize,
    /// Quick play was chosen once on a terminal too small for the whole
    /// board; choosing it again queues anyway.
    confirm_small: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InRoom {
    pub me: PlayerId,
    pub view: Arc<RoomView>,
    /// Esc during a round asks first.
    pub confirm_leave: bool,
}

impl InRoom {
    pub fn my_seat(&self) -> Option<&SeatView> {
        self.view.seat(self.me)
    }

    /// Whether this player has a snake in the round being played.
    pub fn playing(&self) -> bool {
        matches!(
            self.view.phase,
            PhaseView::Countdown { .. } | PhaseView::Running { .. }
        ) && self.my_seat().is_some_and(|s| s.slot.is_some() && s.alive)
    }
}

pub struct App {
    pub cfg: GameConfig,
    pub selected: usize,
    pub game: Option<SoloGame>,
    pub records: Records,
    pub record: u32,
    pub screen: Screen,
    /// Set by the first press of the reset key; a second press resets.
    pub confirm_reset: bool,
    /// The terminal the app is drawn on. Set it with [`Self::resize`].
    pub size: Size,
    /// Only when played over the network.
    pub online: Option<Online>,
    pub input: Input,
}

impl App {
    /// The local game: single player only, starting at the settings.
    pub fn new(cfg: GameConfig, records: Records) -> Self {
        Self {
            cfg,
            selected: 0,
            game: None,
            records,
            record: 0,
            screen: Screen::Menu,
            confirm_reset: false,
            size: Size::new(0, 0),
            online: None,
            input: Input::default(),
        }
    }

    /// The game played over the network, starting at the home menu.
    pub fn online(cfg: GameConfig, records: Records, name: String) -> Self {
        let mut app = Self::new(cfg, records);
        app.screen = Screen::Home { selected: 0 };
        app.online = Some(Online {
            name,
            notice: None,
            room: None,
            waited: Duration::ZERO,
            queue_len: 0,
            confirm_small: false,
        });
        app
    }

    /// How long the runner may wait for a key before calling [`Self::tick`].
    pub fn poll_interval(&self) -> Duration {
        if matches!(self.screen, Screen::Playing) {
            Duration::from_millis(15)
        } else {
            Duration::from_millis(100)
        }
    }

    pub fn resize(&mut self, size: Size) {
        self.size = size;
        self.pause_if_hidden();
    }

    /// The snake must never move while its board isn't on screen.
    fn pause_if_hidden(&mut self) {
        if matches!(self.screen, Screen::Playing)
            && self
                .game
                .as_ref()
                .is_some_and(|g| !ui::board_fits(g.world.width, g.world.height, self.size))
        {
            self.screen = Screen::Paused { selected: 0 };
        }
    }

    /// Advances a running game, or the time spent in the queue, by `dt` of
    /// real time.
    pub fn tick(&mut self, dt: Duration) -> Vec<Effect> {
        let mut effects = Vec::new();
        match self.screen {
            Screen::Playing => {
                if let Some(game) = &mut self.game {
                    game.advance(dt);
                }
                if self.game.as_ref().is_some_and(|g| g.over.is_some()) {
                    let new_record = self.save_record(&mut effects);
                    self.screen = Screen::GameOver { new_record };
                }
            }
            Screen::Finding => {
                if let Some(online) = &mut self.online {
                    online.waited += dt;
                }
            }
            _ => {}
        }
        effects
    }

    fn start_game(&mut self, effects: &mut Vec<Effect>) {
        if let Ok(game) = SoloGame::new(self.cfg.rules.clone()) {
            effects.push(Effect::SaveConfig);
            self.record = self.records.get(&game.rules().record_key());
            self.game = Some(game);
            self.screen = Screen::Playing;
            self.pause_if_hidden();
        }
    }

    fn save_record(&mut self, effects: &mut Vec<Effect>) -> bool {
        let Some(game) = &self.game else { return false };
        let len = game.len() as u32;
        let key = game.rules().record_key();
        let new_record = self.records.update(&key, len);
        if new_record {
            effects.push(Effect::RecordSet { key, best: len });
        }
        self.record = self.record.max(len);
        new_record
    }

    fn quit_to_menu(&mut self, effects: &mut Vec<Effect>) {
        self.save_record(effects);
        self.game = None;
        self.screen = Screen::Menu;
    }

    /// Online, back to the home menu; locally, out of the app.
    fn back_out(&mut self, effects: &mut Vec<Effect>) {
        if self.online.is_some() {
            self.go_home(HomeItem::SinglePlayer, None);
        } else {
            effects.push(Effect::Quit);
        }
    }

    fn go_home(&mut self, at: HomeItem, notice: Option<String>) {
        self.screen = Screen::Home {
            selected: at.index(),
        };
        if let Some(online) = &mut self.online {
            online.notice = notice;
            online.confirm_small = false;
        }
    }

    pub fn handle(&mut self, key: Key) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.handle_key(key, &mut effects);
        self.pause_if_hidden();
        effects
    }

    fn handle_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        // Shift and Caps Lock don't change what a letter key does, except
        // while typing.
        let key = match key {
            Key::Char(c) if !matches!(self.screen, Screen::Input(_)) => {
                Key::Char(c.to_ascii_lowercase())
            }
            key => key,
        };
        if key == Key::CtrlC {
            self.save_record(effects);
            effects.push(Effect::Quit);
            return;
        }
        match self.screen {
            Screen::Home { selected } => self.home_key(selected, key, effects),
            Screen::Menu => {
                let confirm_reset = std::mem::take(&mut self.confirm_reset);
                match key {
                    Key::Esc | Key::Char('q') => self.back_out(effects),
                    Key::Up | Key::Char('w' | 'k') => {
                        self.selected = self.selected.checked_sub(1).unwrap_or(SETTINGS.len() - 1);
                    }
                    Key::Down | Key::Char('s' | 'j') => {
                        self.selected = (self.selected + 1) % SETTINGS.len();
                    }
                    Key::Left | Key::Char('a' | 'h') => self.cfg.adjust(self.selected, -1),
                    Key::Right | Key::Char('d' | 'l') => self.cfg.adjust(self.selected, 1),
                    Key::Char('r') if confirm_reset => self.cfg = GameConfig::default(),
                    Key::Char('r') => self.confirm_reset = true,
                    Key::Enter => self.start_game(effects),
                    _ => {}
                }
            }
            Screen::Playing => {
                if matches!(key, Key::Esc | Key::Char('p')) {
                    self.screen = Screen::Paused { selected: 0 };
                    return;
                }
                if let (Some(dir), Some(game)) = (direction(key), &mut self.game) {
                    game.turn(dir);
                }
            }
            Screen::Paused { selected } => match key {
                Key::Esc | Key::Char('r' | 'p') => self.screen = Screen::Playing,
                Key::Up | Key::Down | Key::Char('w' | 's' | 'k' | 'j') => {
                    self.screen = Screen::Paused {
                        selected: 1 - selected,
                    };
                }
                Key::Enter => {
                    if selected == 0 {
                        self.screen = Screen::Playing;
                    } else {
                        self.quit_to_menu(effects);
                    }
                }
                Key::Char('q') => self.quit_to_menu(effects),
                _ => {}
            },
            Screen::GameOver { .. } => match key {
                Key::Enter | Key::Esc | Key::Char(' ') => {
                    self.game = None;
                    self.screen = Screen::Menu;
                }
                Key::Char('r') => self.start_game(effects),
                Key::Char('q') => {
                    self.game = None;
                    self.back_out(effects);
                }
                _ => {}
            },
            Screen::Input(field) => self.input_key(field, key, effects),
            Screen::Finding => {
                if matches!(key, Key::Esc | Key::Char('q')) {
                    effects.push(Effect::LeaveQueue);
                    self.go_home(HomeItem::QuickPlay, None);
                }
            }
            // Nothing to do until the room answers.
            Screen::Joining { .. } => {}
            Screen::Room => self.room_key(key, effects),
        }
    }

    fn home_key(&mut self, selected: usize, key: Key, effects: &mut Vec<Effect>) {
        let Some(online) = &mut self.online else {
            return;
        };
        let had_notice = online.notice.take().is_some();
        let selected = match key {
            Key::Up | Key::Char('w' | 'k') => selected.checked_sub(1).unwrap_or(HOME.len() - 1),
            Key::Down | Key::Char('s' | 'j') => (selected + 1) % HOME.len(),
            Key::Enter | Key::Char(' ') => return self.choose(HOME[selected], effects),
            Key::Esc | Key::Char('q') if !had_notice => return effects.push(Effect::Quit),
            _ => selected,
        };
        online.confirm_small = false;
        self.screen = Screen::Home { selected };
    }

    fn choose(&mut self, item: HomeItem, effects: &mut Vec<Effect>) {
        let size = self.size;
        let Some(online) = &mut self.online else {
            return;
        };
        match item {
            HomeItem::SinglePlayer => self.screen = Screen::Menu,
            HomeItem::QuickPlay => {
                let rules = quick_rules();
                if !ui::room_fits(rules.width, rules.height, size) && !online.confirm_small {
                    let (w, h) = ui::room_size(rules.width, rules.height);
                    online.notice = Some(format!(
                        "Your terminal is {}×{}; matches need {w}×{h} to show the whole board. \
                         Press Enter again to play anyway.",
                        size.width, size.height
                    ));
                    online.confirm_small = true;
                    return;
                }
                online.confirm_small = false;
                online.waited = Duration::ZERO;
                self.screen = Screen::Finding;
                effects.push(Effect::QuickPlay);
            }
            HomeItem::CreateRoom => {
                self.screen = Screen::Joining { by_code: false };
                effects.push(Effect::CreateRoom(self.cfg.rules.clone()));
            }
            HomeItem::JoinCode => {
                self.input = Input::default();
                self.screen = Screen::Input(Field::Code);
            }
            HomeItem::Nickname => {
                self.input = Input {
                    text: online.name.trim_start_matches('~').to_string(),
                    error: None,
                };
                self.screen = Screen::Input(Field::Nickname);
            }
            HomeItem::Quit => effects.push(Effect::Quit),
        }
    }

    fn input_key(&mut self, field: Field, key: Key, effects: &mut Vec<Effect>) {
        let input = &mut self.input;
        match key {
            Key::Char(c) if field.accepts(c) && input.text.len() < field.max_len() => {
                input.error = None;
                input.text.push(match field {
                    Field::Code => c.to_ascii_uppercase(),
                    Field::Nickname => c,
                });
            }
            Key::Backspace => {
                input.error = None;
                input.text.pop();
            }
            Key::Esc => {
                let at = match field {
                    Field::Code => HomeItem::JoinCode,
                    Field::Nickname => HomeItem::Nickname,
                };
                self.go_home(at, None);
            }
            Key::Enter => match field {
                Field::Code if input.text.len() != field.max_len() => {
                    input.error = Some(format!("Codes are {} characters.", field.max_len()));
                }
                Field::Code => {
                    effects.push(Effect::JoinRoom(input.text.clone()));
                    self.screen = Screen::Joining { by_code: true };
                }
                Field::Nickname if input.text.is_empty() => {
                    input.error = Some("Type a name.".to_string());
                }
                Field::Nickname => {
                    effects.push(Effect::SetNickname(input.text.clone()));
                    self.go_home(HomeItem::Nickname, None);
                }
            },
            _ => {}
        }
    }

    fn room_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        let Some(room) = self.online.as_mut().and_then(|o| o.room.as_mut()) else {
            return;
        };
        let leave = match &room.view.phase {
            PhaseView::Waiting { .. } => match key {
                Key::Enter | Key::Char(' ' | 'r') => {
                    effects.push(Effect::ToggleReady);
                    false
                }
                Key::Esc | Key::Char('q') => true,
                _ => false,
            },
            PhaseView::Countdown { .. } | PhaseView::Running { .. } => {
                let playing = room.playing();
                match (direction(key), key) {
                    (Some(dir), _) if playing => {
                        effects.push(Effect::Steer(dir));
                        false
                    }
                    (_, Key::Char('y')) if room.confirm_leave => true,
                    (_, Key::Char('n') | Key::Esc) if room.confirm_leave => {
                        room.confirm_leave = false;
                        false
                    }
                    (_, Key::Esc | Key::Char('q')) if playing => {
                        room.confirm_leave = true;
                        false
                    }
                    (_, Key::Esc | Key::Char('q')) => true,
                    _ => false,
                }
            }
            PhaseView::Results { .. } => match key {
                Key::Enter | Key::Char(' ') if room.view.kind == RoomKind::Match => {
                    effects.push(Effect::LeaveRoom);
                    self.leave_room();
                    self.choose(HomeItem::QuickPlay, effects);
                    return;
                }
                Key::Esc | Key::Char('q') => true,
                _ => false,
            },
            PhaseView::Closed { .. } => true,
        };
        if leave {
            effects.push(Effect::LeaveRoom);
            self.leave_room();
        }
    }

    fn leave_room(&mut self) {
        let kind = self
            .online
            .as_mut()
            .and_then(|o| o.room.take())
            .map(|r| r.view.kind);
        let at = match kind {
            Some(RoomKind::Private) => HomeItem::CreateRoom,
            _ => HomeItem::QuickPlay,
        };
        self.go_home(at, None);
    }

    /// The name others see, as the server settled it.
    pub fn set_name(&mut self, name: String) {
        if let Some(online) = &mut self.online {
            online.name = name;
        }
    }

    pub fn set_queue_len(&mut self, n: usize) {
        if let Some(online) = &mut self.online {
            online.queue_len = n;
        }
    }

    /// Whether the player is waiting in the queue, so a match is welcome.
    pub fn wants_match(&self) -> bool {
        self.screen == Screen::Finding
    }

    pub fn in_room(&self) -> bool {
        self.online.as_ref().is_some_and(|o| o.room.is_some())
    }

    /// The player is now seated in a room.
    pub fn enter_room(&mut self, me: PlayerId, view: Arc<RoomView>) {
        let Some(online) = &mut self.online else {
            return;
        };
        online.notice = None;
        online.room = Some(InRoom {
            me,
            view,
            confirm_leave: false,
        });
        self.screen = Screen::Room;
    }

    /// The room changed.
    pub fn update_room(&mut self, view: Arc<RoomView>) {
        if let PhaseView::Closed { reason } = &view.phase {
            return self.room_closed(Some(reason.clone()));
        }
        if let Some(room) = self.online.as_mut().and_then(|o| o.room.as_mut()) {
            room.view = view;
            if !room.playing() {
                room.confirm_leave = false;
            }
        }
    }

    /// The room is gone; `reason` says why, if the room said.
    pub fn room_closed(&mut self, reason: Option<String>) {
        if self.in_room() {
            self.leave_room();
            if let Some(online) = &mut self.online {
                online.notice = reason;
            }
        }
    }

    /// Making or joining a private room didn't work.
    pub fn join_failed(&mut self, why: String) {
        match self.screen {
            Screen::Joining { by_code: true } => {
                self.input.error = Some(why);
                self.screen = Screen::Input(Field::Code);
            }
            Screen::Joining { by_code: false } => self.go_home(HomeItem::CreateRoom, Some(why)),
            _ => {}
        }
    }
}

fn direction(key: Key) -> Option<Direction> {
    match key {
        Key::Up | Key::Char('w') => Some(Direction::Up),
        Key::Down | Key::Char('s') => Some(Direction::Down),
        Key::Left | Key::Char('a') => Some(Direction::Left),
        Key::Right | Key::Char('d') => Some(Direction::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use snake_core::round::{Entrant, Round, RoundSettings};

    use super::*;

    fn app() -> App {
        let mut app = App::new(GameConfig::default(), Records::default());
        app.resize(Size::new(120, 40));
        app
    }

    fn online() -> App {
        let mut app = App::online(GameConfig::default(), Records::default(), "~alice".into());
        app.resize(Size::new(120, 40));
        app
    }

    #[test]
    fn menu_d_adjusts_and_r_twice_resets() {
        let mut app = app();
        app.handle(Key::Char('d'));
        app.handle(Key::Char('D'));
        assert_eq!(app.cfg.rules.width, 32);
        app.handle(Key::Char('r'));
        assert!(app.confirm_reset);
        app.handle(Key::Char('x'));
        assert_eq!(app.cfg.rules.width, 32, "any other key cancels the reset");
        app.handle(Key::Char('r'));
        app.handle(Key::Char('r'));
        assert_eq!(app.cfg, GameConfig::default());
    }

    #[test]
    fn ctrl_c_quits_from_every_screen() {
        let mut app = app();
        assert_eq!(app.handle(Key::CtrlC), [Effect::Quit]);
        let effects = app.handle(Key::Enter);
        assert_eq!(effects, [Effect::SaveConfig]);
        assert_eq!(app.screen, Screen::Playing);
        // The first game on these rules is a new record, saved before quitting.
        let len = app.game.as_ref().unwrap().len() as u32;
        let key = GameConfig::default().rules.record_key();
        assert_eq!(
            app.handle(Key::CtrlC),
            [Effect::RecordSet { key, best: len }, Effect::Quit]
        );
    }

    #[test]
    fn pauses_when_the_board_stops_fitting() {
        let mut app = app();
        app.handle(Key::Enter);
        assert_eq!(app.screen, Screen::Playing);
        app.resize(Size::new(40, 15));
        assert_eq!(app.screen, Screen::Paused { selected: 0 });
        app.handle(Key::Esc);
        assert_eq!(
            app.screen,
            Screen::Paused { selected: 0 },
            "can't resume yet"
        );
        app.resize(Size::new(120, 40));
        app.handle(Key::Esc);
        assert_eq!(app.screen, Screen::Playing);
    }

    #[test]
    fn online_single_player_goes_back_home_instead_of_quitting() {
        let mut app = online();
        app.handle(Key::Enter);
        assert_eq!(app.screen, Screen::Menu);
        assert_eq!(app.handle(Key::Char('q')), []);
        assert_eq!(app.screen, Screen::Home { selected: 0 });
        assert_eq!(app.handle(Key::Char('q')), [Effect::Quit]);
    }

    #[test]
    fn codes_are_typed_in_capitals_and_must_be_complete() {
        let mut app = online();
        app.handle(Key::Down);
        app.handle(Key::Down);
        app.handle(Key::Down);
        app.handle(Key::Enter);
        assert_eq!(app.screen, Screen::Input(Field::Code));
        for c in "ab-c".chars() {
            app.handle(Key::Char(c));
        }
        assert_eq!(app.input.text, "ABC");
        app.handle(Key::Enter);
        assert!(app.input.error.is_some());
        for c in "d2k9".chars() {
            app.handle(Key::Char(c));
        }
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
        app.handle(Key::Down);
        assert_eq!(app.handle(Key::Enter), []);
        assert!(app.online.as_ref().unwrap().notice.is_some());
        assert_eq!(app.handle(Key::Enter), [Effect::QuickPlay]);
        assert_eq!(app.screen, Screen::Finding);
        app.tick(Duration::from_secs(2));
        assert_eq!(app.online.as_ref().unwrap().waited, Duration::from_secs(2));
        assert_eq!(app.handle(Key::Esc), [Effect::LeaveQueue]);
    }

    fn matched_view() -> Arc<RoomView> {
        let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let players = [1, 2]
            .map(|id| Entrant {
                id,
                name: format!("~p{id}"),
            })
            .to_vec();
        Arc::new(Round::new_match(settings, 1, players).view())
    }

    #[test]
    fn leaving_a_match_asks_first_while_the_snake_is_alive() {
        let mut app = online();
        app.screen = Screen::Finding;
        app.enter_room(1, matched_view());
        assert_eq!(app.screen, Screen::Room);
        assert_eq!(app.handle(Key::Up), [Effect::Steer(Direction::Up)]);
        assert_eq!(app.handle(Key::Esc), []);
        assert!(
            app.online
                .as_ref()
                .unwrap()
                .room
                .as_ref()
                .unwrap()
                .confirm_leave
        );
        assert_eq!(
            app.handle(Key::Left),
            [Effect::Steer(Direction::Left)],
            "still steering"
        );
        assert_eq!(app.handle(Key::Char('n')), []);
        app.handle(Key::Esc);
        assert_eq!(app.handle(Key::Char('y')), [Effect::LeaveRoom]);
        assert_eq!(app.screen, Screen::Home { selected: 1 });
    }

    #[test]
    fn a_closed_room_sends_the_player_home_with_the_reason() {
        let mut app = online();
        app.enter_room(1, matched_view());
        let mut closed = (*matched_view()).clone();
        closed.phase = PhaseView::Closed {
            reason: "The match is over.".into(),
        };
        app.update_room(Arc::new(closed));
        assert!(!app.in_room());
        assert_eq!(app.screen, Screen::Home { selected: 1 });
        assert_eq!(
            app.online.as_ref().unwrap().notice.as_deref(),
            Some("The match is over.")
        );
    }
}
