//! A ratatui backend for a terminal at the far end of an SSH connection.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// Where the backend's escape sequences collect until they're sent.
#[derive(Clone, Default)]
struct Output(Rc<RefCell<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Draws with crossterm's escape sequences into a buffer, so a whole frame
/// can go out as one SSH packet.
///
/// `CrosstermBackend` would ask the *server's* own terminal for its size
/// and cursor position (ratatui does when resizing or clearing), which is
/// wrong for a remote player and fails outright under systemd. This answers
/// those from the size the client reported instead.
pub struct SshBackend {
    inner: CrosstermBackend<Output>,
    output: Output,
    size: Size,
    cursor: Position,
}

impl SshBackend {
    pub fn new(size: Size) -> Self {
        let output = Output::default();
        Self {
            inner: CrosstermBackend::new(output.clone()),
            output,
            size,
            cursor: Position::ORIGIN,
        }
    }

    /// The client's terminal changed size. Call before `Terminal::resize`.
    pub fn set_size(&mut self, size: Size) {
        self.size = size;
    }

    /// Everything drawn since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut *self.output.0.borrow_mut())
    }
}

impl Backend for SshBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.cursor = position.into();
        self.inner.set_cursor_position(self.cursor)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size::default(),
        })
    }

    /// Nothing to do: the session sends each frame's output in one piece.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::widgets::Paragraph;
    use ratatui::{Terminal, TerminalOptions, Viewport};

    use super::*;

    #[test]
    fn resizing_and_drawing_never_ask_the_local_terminal() {
        // Under `cargo test` stdout isn't the 7x3 terminal claimed here, so
        // any query of the real terminal would give itself away.
        let size = Size::new(7, 3);
        let viewport = Viewport::Fixed(Rect::new(0, 0, size.width, size.height));
        let mut terminal =
            Terminal::with_options(SshBackend::new(size), TerminalOptions { viewport }).unwrap();
        terminal
            .draw(|f| f.render_widget(Paragraph::new("hi"), f.area()))
            .unwrap();
        let first = terminal.backend_mut().take_output();
        assert!(String::from_utf8_lossy(&first).contains("hi"));

        let bigger = Size::new(20, 5);
        terminal.backend_mut().set_size(bigger);
        terminal.resize(Rect::new(0, 0, 20, 5)).unwrap();
        terminal
            .draw(|f| {
                assert_eq!(f.area().as_size(), bigger);
                f.render_widget(Paragraph::new("hello"), f.area());
            })
            .unwrap();
        assert!(String::from_utf8_lossy(&terminal.backend_mut().take_output()).contains("hello"));
        assert!(
            terminal.backend_mut().take_output().is_empty(),
            "taken once"
        );
    }
}
