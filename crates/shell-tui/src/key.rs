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
