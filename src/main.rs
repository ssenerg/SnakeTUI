mod app;
mod board;
mod config;
mod game;
mod records;
mod ui;

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::{App, Screen};

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
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && !app.handle_key(key.code)
        {
            return Ok(());
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
