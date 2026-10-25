mod account;
mod board;
mod dialogs;
mod forms;
mod friends;
mod game;
mod home;
mod leaderboard;
mod menu;
mod room;
mod room_rules;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{App, Screen};
use crate::online::{Field, FormKind, HomeItem};

pub use board::{BoardWidget, SnakeLook};
pub use game::board_fits;
pub use room::{room_fits, room_size};

pub fn draw(frame: &mut Frame, app: &App) {
    // The home menu behind dialogs, with the entry that opened them.
    let items = app.home_items();
    let home = |item: HomeItem| items.iter().position(|i| *i == item).unwrap_or(0);
    match app.screen {
        Screen::Home { selected } => home::draw(frame, app, selected),
        Screen::Input(Field::FriendName) => {
            friends::draw(frame, app, 0);
            dialogs::draw_input(frame, app, Field::FriendName);
        }
        Screen::Input(field) => {
            let item = match field {
                Field::Code => HomeItem::JoinCode,
                _ => HomeItem::Nickname,
            };
            home::draw(frame, app, home(item));
            dialogs::draw_input(frame, app, field);
        }
        Screen::Form(kind) => {
            let item = match kind {
                FormKind::SignUp => HomeItem::SignUp,
                FormKind::SignIn => HomeItem::SignIn,
                FormKind::ChangePassword => HomeItem::Account,
            };
            if kind == FormKind::ChangePassword {
                account::draw(frame, app, 0);
            } else {
                home::draw(frame, app, home(item));
            }
            forms::draw(frame, app, kind);
        }
        Screen::Account { selected } => account::draw(frame, app, selected),
        Screen::Friends { selected } => friends::draw(frame, app, selected),
        Screen::Leaderboard => leaderboard::draw(frame, app),
        Screen::Finding => {
            home::draw(frame, app, home(HomeItem::QuickPlay));
            dialogs::draw_finding(frame, app);
        }
        Screen::Joining { by_code } => {
            let item = if by_code {
                HomeItem::JoinCode
            } else {
                HomeItem::CreateRoom
            };
            home::draw(frame, app, home(item));
            dialogs::draw_joining(frame);
        }
        Screen::Room => room::draw(frame, app),
        Screen::RoomRules { selected } => room_rules::draw(frame, app, selected),
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
    if app.invite_showing() {
        draw_invite(frame, app);
    }
}

/// An invite, along the top of whatever screen is showing.
fn draw_invite(frame: &mut Frame, app: &App) {
    let Some(from) = app.online.as_ref().and_then(|o| o.invite.as_ref()) else {
        return;
    };
    let area = frame.area();
    let rect = Rect {
        height: 3.min(area.height),
        ..centered_rect(area, 56, 3)
    };
    let rect = Rect { y: area.y, ..rect };
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::LightYellow));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let line = Line::from(format!(
        "{from} invites you to play · Enter join · Esc dismiss"
    ))
    .style(
        Style::default()
            .fg(Color::LightYellow)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), inner);
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
