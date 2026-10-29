//! Drawing the app's screens, and helpers for games' own.

mod account;
mod dialogs;
mod forms;
mod friends;
mod home;
mod leaderboard;
mod picker;
mod room;

use std::time::Duration;

use game_api::{Entrant, is_bot};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{App, Screen};
use crate::online::{Field, FormKind, HomeItem};

pub fn draw(frame: &mut Frame, app: &App) {
    if let Some(pictures) = app.pictures() {
        pictures.clear();
    }
    // The home menu behind dialogs, with the entry that opened them.
    let items = app.home_items();
    let home = |item: HomeItem| items.iter().position(|i| *i == item).unwrap_or(0);
    match app.screen {
        Screen::Picker { selected } => picker::draw(frame, app, selected),
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
        Screen::Leaderboard { category } => leaderboard::draw(frame, app, category),
        Screen::JoiningToWatch => {
            friends::draw(frame, app, 0);
            dialogs::draw_joining(frame);
        }
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
        Screen::Game => app.ui().draw(frame, &app.ctx()),
    }
    if app.invite_showing() {
        draw_invite(frame, app);
    }
    // Pictures show only where nothing was drawn over them.
    if let Some(pictures) = app.pictures() {
        pictures.settle(frame.buffer_mut());
    }
}

/// An invite, along the top of whatever screen is showing.
fn draw_invite(frame: &mut Frame, app: &App) {
    let Some(invite) = &app.online.invite else {
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
    // Which game, when it isn't the one open.
    let game = app
        .games()
        .find(|g| g.id() == invite.game && Some(invite.game) != app.game())
        .map_or(String::new(), |g| format!(" {}", g.title()));
    let line = Line::from(format!(
        "{} invites you to play{game} · Enter join · Esc dismiss",
        invite.from
    ))
    .style(
        Style::default()
            .fg(Color::LightYellow)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), inner);
}

/// A `width`×`height` rectangle in the middle of `area`, cut down to fit.
pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// A bordered box in the middle of the screen, over whatever is there;
/// returns the space inside it.
pub fn dialog(frame: &mut Frame, title: &str, border: Color, width: u16, height: u16) -> Rect {
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

/// The open game's name, big, across the top of `area`.
pub(crate) fn draw_logo(frame: &mut Frame, app: &App, area: Rect) {
    let (logo, color) = app.ui().logo();
    let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let logo: Vec<Line> = logo.iter().map(|l| Line::from(*l).style(style)).collect();
    frame.render_widget(Paragraph::new(logo).alignment(Alignment::Center), area);
}

pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Whole seconds left, counting a part of one as one.
pub fn secs(d: Duration) -> u64 {
    d.as_secs_f64().ceil() as u64
}

/// A player's name, followed by `tag` if they're a bot.
pub fn named(who: &Entrant, tag: Option<&str>) -> String {
    match tag {
        Some(tag) if is_bot(who.id) => format!("{} {tag}", who.name),
        _ => who.name.clone(),
    }
}

/// A round trip as shown: "42 ms", or "1.2 s" from a second on. `tight`
/// leaves out the space.
pub fn ping_text(ping: Duration, tight: bool) -> String {
    let gap = if tight { "" } else { " " };
    match ping.as_millis() {
        ms @ 0..1000 => format!("{}{gap}ms", ms.max(1)),
        _ => format!("{:.1}{gap}s", ping.as_secs_f64()),
    }
}

/// Green while moves feel instant, yellow once they lag, red when they lag
/// badly.
pub fn ping_style(ping: Duration) -> Style {
    let color = match ping.as_millis() {
        0..80 => Color::LightGreen,
        80..200 => Color::Yellow,
        _ => Color::LightRed,
    };
    Style::default().fg(color)
}
