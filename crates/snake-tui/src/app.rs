use std::time::Duration;

use ratatui::layout::Size;
use snake_core::config::{GameConfig, Rules};
use snake_core::geom::Direction;
use snake_core::records::Records;
use snake_core::settings::SETTINGS;
use snake_core::solo::SoloGame;

use crate::key::Key;
use crate::online::{Field, Form, FormKind, Online, Secret};
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    /// Online: the main menu, at the [`App::home_items`] entry `selected`.
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
    /// Online: filling in [`App::form`].
    Form(FormKind),
    /// Online: the signed-in account.
    Account {
        selected: usize,
    },
    /// Online: friends and friend requests.
    Friends {
        selected: usize,
    },
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
    /// The host's pick for the room's next rounds: a tick, in milliseconds.
    RoomSpeed(u64),
    LeaveRoom,
    /// Answer with [`App::set_name`].
    SetNickname(String),
    /// Answer with [`App::signed_up`] or [`App::form_failed`].
    SignUp {
        username: String,
        password: Secret,
    },
    /// Answer with [`App::signed_in`] or [`App::form_failed`].
    SignIn {
        username: String,
        password: Secret,
        /// Remember this machine's key for the account.
        remember: bool,
    },
    /// Answer with [`App::signed_out`].
    SignOut,
    /// Answer with [`App::password_changed`] or [`App::form_failed`].
    ChangePassword(Secret),
    /// Stop remembering the account by this key; answer with
    /// [`App::set_keys`].
    ForgetKey(String),
    /// Answer with [`App::set_keys`].
    LoadAccount,
    /// Answer with [`App::set_friends`].
    LoadFriends,
    AddFriend(String),
    AnswerFriend {
        name: String,
        accept: bool,
    },
    /// Ends a friendship, or takes back a request.
    RemoveFriend(String),
    /// Invite this friend to the private room the player is in, or to a
    /// new one.
    Invite(String),
    AcceptInvite,
    DismissInvite,
}

/// What's being typed on an input screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    pub error: Option<String>,
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
    pub form: Form,
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
            form: Form::default(),
        }
    }

    /// The game played over the network, starting at the home menu, as the
    /// guest `name`. `has_key` says whether the player connected with an
    /// SSH key, which an account can then remember.
    pub fn online(cfg: GameConfig, records: Records, name: String, has_key: bool) -> Self {
        let mut app = Self::new(cfg, records);
        app.screen = Screen::Home { selected: 0 };
        app.online = Some(Online::new(name, has_key));
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

    pub(crate) fn save_record(&mut self, effects: &mut Vec<Effect>) -> bool {
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
            self.go_home(crate::online::HomeItem::SinglePlayer, None);
        } else {
            effects.push(Effect::Quit);
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
        let typing = matches!(self.screen, Screen::Input(_) | Screen::Form(_));
        let key = match key {
            Key::Char(c) if !typing => Key::Char(c.to_ascii_lowercase()),
            key => key,
        };
        if key == Key::CtrlC {
            self.save_record(effects);
            effects.push(Effect::Quit);
            return;
        }
        if self.invite_key(key, effects) {
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
            Screen::Finding => self.finding_key(key, effects),
            // Nothing to do until the room answers.
            Screen::Joining { .. } => {}
            Screen::Room => self.room_key(key, effects),
            Screen::Form(kind) => self.form_key(kind, key, effects),
            Screen::Account { selected } => self.account_key(selected, key, effects),
            Screen::Friends { selected } => self.friends_key(selected, key, effects),
        }
    }
}

pub(crate) fn direction(key: Key) -> Option<Direction> {
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
    use super::*;

    fn app() -> App {
        let mut app = App::new(GameConfig::default(), Records::default());
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
}
