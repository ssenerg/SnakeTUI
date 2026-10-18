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
        // The snake must never move while its board isn't on screen.
        let size = terminal.size()?;
        if matches!(app.screen, Screen::Playing)
            && app.game.as_ref().is_some_and(|g| !ui::board_fits(g, size))
        {
            app.screen = Screen::Paused { selected: 0 };
        }

        terminal.draw(|frame| ui::draw(frame, &app))?;

        let timeout = if matches!(app.screen, Screen::Playing) {
            Duration::from_millis(15)
        } else {
            Duration::from_millis(100)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if !app.handle_key(key) {
                        return Ok(());
                    }
                }
                // Re-check the fit before the game moves at the new size.
                Event::Resize(..) => continue,
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
