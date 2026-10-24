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
