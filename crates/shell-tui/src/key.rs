/// A key press, independent of where it came from: a local terminal or an
/// SSH connection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
    Char(char),
    CtrlC,
    /// The left mouse button, at a cell of the screen. Only screens that
    /// ask for the mouse get these: see [`crate::GameUi::wants_mouse`].
    Mouse(Mouse),
}

/// The left button went down or came up at column `x`, row `y`, from the
/// top left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mouse {
    pub kind: MouseKind,
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseKind {
    Press,
    Release,
}

impl Mouse {
    /// Whether it's within `area`.
    pub fn within(self, area: ratatui::layout::Rect) -> bool {
        area.contains(ratatui::layout::Position::new(self.x, self.y))
    }
}

impl Key {
    /// Moves `selected` up or down a list of `len` rows, wrapping around;
    /// `None` if this key doesn't move it.
    pub fn step(self, selected: usize, len: usize) -> Option<usize> {
        match self {
            Key::Up | Key::Char('w' | 'k') => Some(selected.checked_sub(1).unwrap_or(len - 1)),
            Key::Down | Key::Char('s' | 'j') => Some((selected + 1) % len),
            _ => None,
        }
    }

    /// Letters as lower case: Shift and Caps Lock don't change what a
    /// letter key does, except while typing.
    pub fn lower(self) -> Key {
        match self {
            Key::Char(c) => Key::Char(c.to_ascii_lowercase()),
            key => key,
        }
    }
}
