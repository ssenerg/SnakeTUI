use ratatui::crossterm::event::KeyCode;

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

    pub fn handle_key(&mut self, code: KeyCode) -> bool {
        match self.screen {
            Screen::Menu => match code {
                KeyCode::Esc | KeyCode::Char('q') => return false,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.selected = self.selected.checked_sub(1).unwrap_or(SETTINGS.len() - 1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1) % SETTINGS.len();
                }
                KeyCode::Left | KeyCode::Char('h') => self.cfg.adjust(self.selected, -1),
                KeyCode::Right | KeyCode::Char('l') => self.cfg.adjust(self.selected, 1),
                KeyCode::Char('d') => self.cfg = GameConfig::default(),
                KeyCode::Enter => self.start_game(),
                _ => {}
            },
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
                KeyCode::Up | KeyCode::Down | KeyCode::Char('k') | KeyCode::Char('j') => {
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
