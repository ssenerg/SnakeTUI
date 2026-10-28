//! Snake's own screens: single player, a room's rules, and its rounds.

mod board;
mod dialogs;
mod game;
mod menu;
mod room;
mod room_rules;

use ratatui::Frame;
use shell_tui::Ctx;

use crate::app::{SnakeScreen, SnakeUi};

pub use board::{BoardWidget, SnakeLook};
pub use game::board_fits;
pub(crate) use menu::LOGO;
pub(crate) use room::draw_round;
pub use room::{room_fits, room_size};

pub(crate) fn draw(frame: &mut Frame, ui: &SnakeUi, ctx: &Ctx) {
    match ui.screen {
        SnakeScreen::Menu => menu::draw(frame, ui, ctx.online),
        SnakeScreen::Playing => game::draw(frame, ui),
        SnakeScreen::Paused { selected } => {
            game::draw(frame, ui);
            dialogs::draw_pause(frame, selected);
        }
        SnakeScreen::GameOver { new_record } => {
            game::draw(frame, ui);
            dialogs::draw_game_over(frame, ui, new_record);
        }
        SnakeScreen::RoomRules { selected } => room_rules::draw(frame, ui, ctx, selected),
    }
}
