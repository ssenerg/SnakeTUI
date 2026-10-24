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

/// Colors for other players' snakes, in the order to hand them out: none
/// that this player's theme uses for food, poison, walls, the background or
/// their own snake, and nothing too dark to see. Repeats when there are
/// more opponents than colors; numbers on the heads still tell them apart.
pub fn opponent_colors(theme: &Theme) -> Vec<Color> {
    // cyan, blue, yellow, white, light magenta, red, light cyan, light blue,
    // green, magenta, light yellow, light red, light green, gray
    const PREFERENCE: [usize; 14] = [10, 6, 4, 12, 9, 0, 11, 7, 2, 8, 5, 1, 3, 13];
    let taken = [
        ColorSlot::Head,
        ColorSlot::Body,
        ColorSlot::Food,
        ColorSlot::SuperFood,
        ColorSlot::Poison,
        ColorSlot::Wall,
        ColorSlot::Background,
    ]
    .map(|slot| theme.color_index(slot));
    let free: Vec<Color> = PREFERENCE
        .iter()
        .filter(|i| !taken.contains(i))
        .map(|&i| PALETTE[i])
        .collect();
    if free.is_empty() {
        vec![Color::White]
    } else {
        free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opponents_never_look_like_food_walls_or_you() {
        let theme = Theme::default();
        let colors = opponent_colors(&theme);
        for slot in [
            ColorSlot::Head,
            ColorSlot::Body,
            ColorSlot::Food,
            ColorSlot::SuperFood,
            ColorSlot::Poison,
            ColorSlot::Wall,
            ColorSlot::Background,
        ] {
            assert!(!colors.contains(&theme_color(&theme, slot)));
        }
        assert!(colors.len() >= 7, "a different color for each opponent");
    }
}
