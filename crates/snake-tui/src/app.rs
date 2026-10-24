use std::time::Duration;

use ratatui::layout::Size;
use snake_core::config::GameConfig;
use snake_core::geom::Direction;
use snake_core::records::Records;
use snake_core::settings::SETTINGS;
use snake_core::solo::SoloGame;

use crate::key::Key;
use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Menu,
    Playing,
    Paused { selected: usize },
    GameOver { new_record: bool },
}

/// Work the app leaves to whoever runs it, in the order given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Persist [`App::cfg`].
    SaveConfig,
    /// A new best was just set in [`App::records`]. By the time this is
    /// handled the game may be gone, so it says which record changed.
    RecordSet { key: String, best: u32 },
    /// Leave the app.
    Quit,
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
}

impl App {
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
        }
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

    /// Advances a running game by `dt` of real time.
    pub fn tick(&mut self, dt: Duration) -> Vec<Effect> {
        let mut effects = Vec::new();
        if matches!(self.screen, Screen::Playing) {
            if let Some(game) = &mut self.game {
                game.advance(dt);
            }
            if self.game.as_ref().is_some_and(|g| g.over.is_some()) {
                let new_record = self.save_record(&mut effects);
                self.screen = Screen::GameOver { new_record };
            }
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

    pub fn handle(&mut self, key: Key) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.handle_key(key, &mut effects);
        self.pause_if_hidden();
        effects
    }

    fn handle_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        // Shift and Caps Lock don't change what a letter key does.
        let key = match key {
            Key::Char(c) => Key::Char(c.to_ascii_lowercase()),
            key => key,
        };
        if key == Key::CtrlC {
            self.save_record(effects);
            effects.push(Effect::Quit);
            return;
        }
        match self.screen {
            Screen::Menu => {
                let confirm_reset = std::mem::take(&mut self.confirm_reset);
                match key {
                    Key::Esc | Key::Char('q') => effects.push(Effect::Quit),
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
                let dir = match key {
                    Key::Esc | Key::Char('p') => {
                        self.screen = Screen::Paused { selected: 0 };
                        return;
                    }
                    Key::Up | Key::Char('w') => Some(Direction::Up),
                    Key::Down | Key::Char('s') => Some(Direction::Down),
                    Key::Left | Key::Char('a') => Some(Direction::Left),
                    Key::Right | Key::Char('d') => Some(Direction::Right),
                    _ => None,
                };
                if let (Some(dir), Some(game)) = (dir, &mut self.game) {
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
                Key::Char('q') => effects.push(Effect::Quit),
                _ => {}
            },
        }
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
