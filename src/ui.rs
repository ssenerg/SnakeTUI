use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use crate::config::{CONFIG_ITEMS, GameConfig, palette_color};
use crate::game::{FoodKind, Game, GameOverReason};
use crate::{App, Screen};

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen {
        Screen::Config => draw_config(frame, app),
        Screen::Playing => draw_game(frame, app),
        Screen::Paused { selected } => {
            draw_game(frame, app);
            draw_pause(frame, selected);
        }
        Screen::GameOver { new_record } => {
            draw_game(frame, app);
            draw_game_over(frame, app, new_record);
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

fn draw_config(frame: &mut Frame, app: &App) {
    let [title_area, list_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new("S N A K E")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD)),
        title_area,
    );

    let list_rect = centered_rect(list_area, 56.min(list_area.width), list_area.height);
    let visible = list_rect.height as usize;
    let offset = if visible == 0 {
        0
    } else if app.selected >= visible {
        app.selected + 1 - visible
    } else {
        0
    };

    let mut lines = Vec::new();
    for i in offset..CONFIG_ITEMS.min(offset + visible.max(1)) {
        let selected = i == app.selected;
        let style = if selected {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let marker = if selected { "▶ " } else { "  " };
        let mut spans = vec![Span::styled(
            format!("{marker}{:<26}", GameConfig::item_label(i)),
            style,
        )];
        if let Some(color) = app.cfg.item_swatch(i) {
            spans.push(Span::styled("██ ", Style::default().fg(color)));
        }
        spans.push(Span::styled(format!("◄ {} ►", app.cfg.item_value(i)), style));
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), list_rect);

    let record = app.records.get(&app.cfg.record_key());
    let footer = vec![
        Line::from(format!("record for this config: {record}")).style(Style::default().fg(Color::Cyan)),
        Line::from("↑↓ select   ←→ change   Enter play   q quit")
            .style(Style::default().fg(Color::DarkGray)),
    ];
    frame.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        footer_area,
    );
}

fn draw_game(frame: &mut Frame, app: &App) {
    let Some(game) = &app.game else { return };
    let [hud_area, board_outer, help_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let len = game.len();
    let record = app.record.max(len as u32);
    let secs = game.clock.as_secs();
    let mut hud = format!(
        "Length: {len}    Record: {record}    Time: {:02}:{:02}",
        secs / 60,
        secs % 60
    );
    if game.boost_active() {
        hud.push_str("    ⚡ BOOST");
    }
    frame.render_widget(
        Paragraph::new(hud)
            .alignment(Alignment::Center)
            .style(Style::default().add_modifier(Modifier::BOLD)),
        hud_area,
    );
    let (help, help_style) = if game.started {
        (
            "←↑↓→ / WASD move   Esc pause",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        (
            "press an arrow key to start",
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        )
    };
    frame.render_widget(
        Paragraph::new(help)
            .alignment(Alignment::Center)
            .style(help_style),
        help_area,
    );

    let need_w = game.width * 2 + 2;
    let need_h = game.height + 2;
    if board_outer.width < need_w || board_outer.height < need_h {
        frame.render_widget(
            Paragraph::new(format!("terminal too small — need at least {need_w}x{need_h}"))
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Red)),
            board_outer,
        );
        return;
    }
    let rect = centered_rect(board_outer, need_w, need_h);
    let block = Block::bordered().border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(board_lines(game)), inner);
}

fn board_lines(game: &Game) -> Vec<Line<'static>> {
    #[derive(Clone, Copy)]
    enum K {
        Empty,
        Wall,
        Food,
        Super,
        Poison,
        Body,
        Head,
    }

    let cfg = &game.cfg;
    let (w, h) = (game.width as usize, game.height as usize);
    let mut grid = vec![K::Empty; w * h];
    for y in 0..game.height {
        for x in 0..game.width {
            if game.wall_at(x, y) {
                grid[y as usize * w + x as usize] = K::Wall;
            }
        }
    }
    for food in &game.foods {
        grid[food.pos.y as usize * w + food.pos.x as usize] = match food.kind {
            FoodKind::Normal => K::Food,
            FoodKind::Super => K::Super,
            FoodKind::Poison => K::Poison,
        };
    }
    for p in game.snake.iter().skip(1) {
        grid[p.y as usize * w + p.x as usize] = K::Body;
    }
    if let Some(head) = game.snake.front() {
        grid[head.y as usize * w + head.x as usize] = K::Head;
    }

    let bg = palette_color(cfg.color_bg);
    let mut lines = Vec::with_capacity(h);
    for y in 0..h {
        let mut spans = Vec::with_capacity(w);
        for x in 0..w {
            let (text, fg, bold) = match grid[y * w + x] {
                K::Empty => ("  ", bg, false),
                K::Wall => ("██", palette_color(cfg.color_wall), false),
                K::Head => ("██", palette_color(cfg.color_head), false),
                K::Body => ("██", palette_color(cfg.color_body), false),
                K::Food => ("██", palette_color(cfg.color_food), false),
                K::Super => ("██", palette_color(cfg.color_super), false),
                K::Poison => ("██", palette_color(cfg.color_poison), false),
            };
            let mut style = Style::default().fg(fg).bg(bg);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(text, style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn draw_pause(frame: &mut Frame, selected: usize) {
    let rect = centered_rect(frame.area(), 34, 8);
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" PAUSED ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let item = |label: &str, active: bool| {
        if active {
            Line::from(format!("▶ {label}"))
                .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        } else {
            Line::from(format!("  {label}"))
        }
    };
    let lines = vec![
        Line::default(),
        item("Resume", selected == 0),
        item("Quit to menu", selected == 1),
        Line::default(),
        Line::from("↑↓ + Enter · Esc resume").style(Style::default().fg(Color::DarkGray)),
    ];
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        inner,
    );
}

fn draw_game_over(frame: &mut Frame, app: &App, new_record: bool) {
    let Some(game) = &app.game else { return };
    let reason = match game.over {
        Some(GameOverReason::HitWall) => "You hit a wall!",
        Some(GameOverReason::HitSelf) => "You ate yourself!",
        Some(GameOverReason::Poisoned) => "You ate poison!",
        Some(GameOverReason::MaxLength) => "Maximum length reached — you win!",
        None => "",
    };
    let rect = centered_rect(frame.area(), 44, 10);
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" GAME OVER ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let secs = game.clock.as_secs();
    let mut lines = vec![
        Line::default(),
        Line::from(reason).style(Style::default().add_modifier(Modifier::BOLD)),
        Line::from(format!(
            "Length: {}   Time: {:02}:{:02}",
            game.len(),
            secs / 60,
            secs % 60
        )),
        Line::from(format!("Record: {}", app.record)),
    ];
    if new_record {
        lines.push(
            Line::from("★ NEW RECORD ★")
                .style(Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD)),
        );
    }
    lines.push(Line::default());
    lines.push(
        Line::from("Enter menu · q quit").style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        inner,
    );
}
