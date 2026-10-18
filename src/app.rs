use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{GameConfig, SETTINGS};
use crate::game::{Direction, Game};
use crate::records::Records;

#[derive(Clone, Copy)]
pub enum Screen {
    Menu,
    Playing,
    Paused { selected: usize },
    GameOver { new_record: bool },
}

pub struct App {
    pub cfg: GameConfig,
    pub selected: usize,
    pub game: Option<Game>,
    pub records: Records,
    pub record: u32,
    pub screen: Screen,
    /// Set by the first press of the reset key; a second press resets.
    pub confirm_reset: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            cfg: GameConfig::load(),
            selected: 0,
            game: None,
            records: Records::load(),
            record: 0,
            screen: Screen::Menu,
            confirm_reset: false,
        }
    }

    fn start_game(&mut self) {
        if let Ok(game) = Game::new(self.cfg.clone()) {
            self.cfg.save();
            self.record = self.records.get(&self.cfg.record_key());
            self.game = Some(game);
            self.screen = Screen::Playing;
        }
    }

    pub fn save_record(&mut self) -> bool {
        let Some(game) = &self.game else { return false };
        let len = game.len() as u32;
        let new_record = self.records.update(&self.cfg.record_key(), len);
        self.record = self.record.max(len);
        new_record
    }

    fn quit_to_menu(&mut self) {
        self.save_record();
        self.game = None;
        self.screen = Screen::Menu;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            // Raw mode delivers Ctrl+C as a key press, not as a signal.
            if matches!(key.code, KeyCode::Char('c' | 'C')) {
                self.save_record();
                return false;
            }
            return true;
        }
        // Shift and Caps Lock don't change what a letter key does.
        let code = match key.code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            code => code,
        };
        match self.screen {
            Screen::Menu => {
                let confirm_reset = std::mem::take(&mut self.confirm_reset);
                match code {
                    KeyCode::Esc | KeyCode::Char('q') => return false,
                    KeyCode::Up | KeyCode::Char('w' | 'k') => {
                        self.selected = self.selected.checked_sub(1).unwrap_or(SETTINGS.len() - 1);
                    }
                    KeyCode::Down | KeyCode::Char('s' | 'j') => {
                        self.selected = (self.selected + 1) % SETTINGS.len();
                    }
                    KeyCode::Left | KeyCode::Char('a' | 'h') => self.cfg.adjust(self.selected, -1),
                    KeyCode::Right | KeyCode::Char('d' | 'l') => self.cfg.adjust(self.selected, 1),
                    KeyCode::Char('r') if confirm_reset => self.cfg = GameConfig::default(),
                    KeyCode::Char('r') => self.confirm_reset = true,
                    KeyCode::Enter => self.start_game(),
                    _ => {}
                }
            }
            Screen::Playing => {
                let dir = match code {
                    KeyCode::Esc | KeyCode::Char('p') => {
                        self.screen = Screen::Paused { selected: 0 };
                        return true;
                    }
                    KeyCode::Up | KeyCode::Char('w') => Some(Direction::Up),
                    KeyCode::Down | KeyCode::Char('s') => Some(Direction::Down),
                    KeyCode::Left | KeyCode::Char('a') => Some(Direction::Left),
                    KeyCode::Right | KeyCode::Char('d') => Some(Direction::Right),
                    _ => None,
                };
                if let (Some(dir), Some(game)) = (dir, &mut self.game) {
                    game.turn(dir);
                }
            }
            Screen::Paused { selected } => match code {
                KeyCode::Esc | KeyCode::Char('r') | KeyCode::Char('p') => {
                    self.screen = Screen::Playing;
                }
                KeyCode::Up | KeyCode::Down | KeyCode::Char('w' | 's' | 'k' | 'j') => {
                    self.screen = Screen::Paused {
                        selected: 1 - selected,
                    };
                }
                KeyCode::Enter => {
                    if selected == 0 {
                        self.screen = Screen::Playing;
                    } else {
                        self.quit_to_menu();
                    }
                }
                KeyCode::Char('q') => self.quit_to_menu(),
                _ => {}
            },
            Screen::GameOver { .. } => match code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') => {
                    self.game = None;
                    self.screen = Screen::Menu;
                }
                KeyCode::Char('r') => self.start_game(),
                KeyCode::Char('q') => return false,
                _ => {}
            },
        }
        true
    }
}
