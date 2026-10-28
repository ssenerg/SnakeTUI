mod persist;

use std::time::Instant;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use shell_tui::{Ctx, GameFx, GameUi, Key, Next};
use snake_tui::SnakeUi;

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

/// Saves what the game asks to.
fn apply(snake: &SnakeUi, fx: Vec<GameFx>) {
    for fx in fx {
        match fx {
            GameFx::SavePrefs(_) => persist::save_config(&snake.cfg),
            GameFx::Record { .. } => persist::save_records(&snake.records),
            // Multiplayer: the local game never plays in rooms.
            GameFx::Play(_) | GameFx::Settings(_) => {}
        }
    }
}

fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut snake = SnakeUi::new(persist::load_config(), persist::load_records());
    let mut last_frame = Instant::now();

    loop {
        let ctx = Ctx::alone(terminal.size()?);
        snake.resize(ctx.size);
        terminal.draw(|frame| snake.draw(frame, &ctx))?;

        if event::poll(snake.poll_interval())? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // Ctrl+C keeps the record of a game in progress.
                    if let Some(Key::CtrlC) = to_key(key) {
                        let fx = snake.quit();
                        apply(&snake, fx);
                        return Ok(());
                    }
                    if let Some(key) = to_key(key) {
                        let reply = snake.key(key, &ctx);
                        apply(&snake, reply.fx);
                        // Home is the way out on its own.
                        if reply.next == Next::Home {
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
        let fx = snake.tick(dt);
        apply(&snake, fx);
    }
}

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}
