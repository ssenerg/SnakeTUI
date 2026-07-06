mod dialogs;
mod game;
mod menu;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Clear};

use crate::app::{App, Screen};

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen {
        Screen::Menu => menu::draw(frame, app),
        Screen::Playing => game::draw(frame, app),
        Screen::Paused { selected } => {
            game::draw(frame, app);
            dialogs::draw_pause(frame, selected);
        }
        Screen::GameOver { new_record } => {
            game::draw(frame, app);
            dialogs::draw_game_over(frame, app, new_record);
        }
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn dialog(frame: &mut Frame, title: &str, border: Color, width: u16, height: u16) -> Rect {
    let rect = centered_rect(frame.area(), width, height);
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title.to_string())
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(border));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}
