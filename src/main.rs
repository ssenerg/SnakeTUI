mod config;
mod game;
mod records;
mod ui;

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use config::{CONFIG_ITEMS, GameConfig};
use game::{Direction, Game};
use records::Records;

#[derive(Clone, Copy)]
pub enum Screen {
    Config,
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
    fn new() -> Self {
        Self {
            cfg: GameConfig::load(),
            selected: 0,
            game: None,
            records: Records::load(),
            record: 0,
            screen: Screen::Config,
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

    fn save_record(&mut self) -> bool {
        let Some(game) = &self.game else { return false };
        let len = game.len() as u32;
        let new_record = self.records.update(&self.cfg.record_key(), len);
        self.record = self.record.max(len);
        new_record
    }

    fn quit_to_menu(&mut self) {
        self.save_record();
        self.game = None;
        self.screen = Screen::Config;
    }

    /// Returns false when the app should exit.
    fn handle_key(&mut self, code: KeyCode) -> bool {
        match self.screen {
            Screen::Config => match code {
                KeyCode::Esc | KeyCode::Char('q') => return false,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.selected = self.selected.checked_sub(1).unwrap_or(CONFIG_ITEMS - 1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1) % CONFIG_ITEMS;
                }
                KeyCode::Left | KeyCode::Char('h') => self.cfg.adjust(self.selected, -1),
                KeyCode::Right | KeyCode::Char('l') => self.cfg.adjust(self.selected, 1),
                KeyCode::Enter => self.start_game(),
                _ => {}
            },
            Screen::Playing => {
                let dir = match code {
                    KeyCode::Esc => {
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
                    // Direction keys move immediately, ahead of the tick timer.
                    game.turn(dir);
                }
            }
            Screen::Paused { selected } => match code {
                KeyCode::Esc | KeyCode::Char('r') => self.screen = Screen::Playing,
                KeyCode::Up | KeyCode::Down | KeyCode::Char('k') | KeyCode::Char('j') => {
                    self.screen = Screen::Paused { selected: 1 - selected };
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
                    self.screen = Screen::Config;
                }
                KeyCode::Char('q') => return false,
                _ => {}
            },
        }
        true
    }
}

fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut app = App::new();
    let mut last_frame = Instant::now();

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        let timeout = if matches!(app.screen, Screen::Playing) {
            Duration::from_millis(15)
        } else {
            Duration::from_millis(100)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if !app.handle_key(key.code) {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        let now = Instant::now();
        let dt = now - last_frame;
        last_frame = now;

        if matches!(app.screen, Screen::Playing) {
            if let Some(game) = &mut app.game {
                game.advance(dt);
            }
            if app.game.as_ref().is_some_and(|g| g.over.is_some()) {
                let new_record = app.save_record();
                app.screen = Screen::GameOver { new_record };
            }
        }
    }
}

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}
