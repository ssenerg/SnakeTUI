mod persist;

use std::time::Instant;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use snake_tui::{App, Effect, Key};

/// The app's key for a terminal key press, or `None` for keys it ignores.
fn to_key(key: KeyEvent) -> Option<Key> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        // Raw mode delivers Ctrl+C as a key press, not as a signal.
        return matches!(key.code, KeyCode::Char('c' | 'C')).then_some(Key::CtrlC);
    }
    Some(match key.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char(c) => Key::Char(c),
        _ => return None,
    })
}

/// Carries out the app's effects; returns `false` once it should quit.
fn apply(app: &App, effects: Vec<Effect>) -> bool {
    for effect in effects {
        match effect {
            Effect::SaveConfig => persist::save_config(&app.cfg),
            Effect::SaveRecords => persist::save_records(&app.records),
            Effect::Quit => return false,
        }
    }
    true
}

fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut app = App::new(persist::load_config(), persist::load_records());
    let mut last_frame = Instant::now();

    loop {
        app.resize(terminal.size()?);
        terminal.draw(|frame| snake_tui::ui::draw(frame, &app))?;

        if event::poll(app.poll_interval())? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(key) = to_key(key) {
                        let effects = app.handle(key);
                        if !apply(&app, effects) {
                            return Ok(());
                        }
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
        let effects = app.tick(dt);
        if !apply(&app, effects) {
            return Ok(());
        }
    }
}

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}
