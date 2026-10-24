use ratatui::style::Color;
use snake_core::config::{COLOR_NAMES, ColorSlot, Theme};

/// Terminal color for each entry of [`COLOR_NAMES`], in the same order.
pub const PALETTE: [Color; 16] = [
    Color::Red,
    Color::LightRed,
    Color::Green,
    Color::LightGreen,
    Color::Yellow,
    Color::LightYellow,
    Color::Blue,
    Color::LightBlue,
    Color::Magenta,
    Color::LightMagenta,
    Color::Cyan,
    Color::LightCyan,
    Color::White,
    Color::Gray,
    Color::DarkGray,
    Color::Black,
];

const _: () = assert!(PALETTE.len() == COLOR_NAMES.len());

pub fn color(idx: usize) -> Color {
    PALETTE[idx % PALETTE.len()]
}

pub fn theme_color(theme: &Theme, slot: ColorSlot) -> Color {
    PALETTE[theme.color_index(slot)]
}
