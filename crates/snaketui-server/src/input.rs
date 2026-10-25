//! Turns the bytes a player's terminal sends into [`Key`]s, and keeps how
//! fast anyone sends them within reason.

use std::time::Instant;

use snake_tui::Key;

const ESC: u8 = 0x1b;
/// A sequence longer than this is garbage, not a key.
const MAX_SEQUENCE: usize = 32;
/// Asks the terminal how it is (device status report). Every terminal since
/// the VT100 answers at once with `ESC [ 0 n`, which no key sends, so the
/// wait for the answer is the player's round trip.
pub const STATUS_QUERY: &[u8] = b"\x1b[5n";

/// Parses terminal input. Packets can end in the middle of an escape
/// sequence, so an unfinished one is kept until the rest arrives.
///
/// A lone ESC is ambiguous: the Esc key, or the start of a sequence still
/// on its way. [`Self::pending_esc`] reports that case, and the caller
/// calls [`Self::flush`] if nothing follows soon.
#[derive(Default)]
pub struct KeyParser {
    pending: Vec<u8>,
    /// The last byte was CR, so a following LF is part of the same Enter.
    after_cr: bool,
    /// Answers to [`STATUS_QUERY`] since [`Self::take_reports`].
    reports: u32,
}

impl KeyParser {
    /// Parses `bytes`, appending the keys they contain to `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Key>) {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(bytes);
        let mut i = 0;
        while i < buf.len() {
            if buf[i] == ESC {
                match parse_escape(&buf[i..]) {
                    Escape::Incomplete => {
                        self.pending = buf[i..].to_vec();
                        return;
                    }
                    Escape::Key(key, len) => {
                        out.extend(key);
                        i += len;
                    }
                    Escape::Report(len) => {
                        self.reports += 1;
                        i += len;
                    }
                }
                self.after_cr = false;
                continue;
            }
            let b = buf[i];
            match b {
                b'\r' => out.push(Key::Enter),
                b'\n' if !self.after_cr => out.push(Key::Enter),
                0x03 => out.push(Key::CtrlC),
                0x7f | 0x08 => out.push(Key::Backspace),
                0x20..=0x7e => out.push(Key::Char(b as char)),
                // Other control keys and non-ASCII text mean nothing here.
                _ => {}
            }
            self.after_cr = b == b'\r';
            i += 1;
        }
    }

    /// Whether input ended on a lone ESC that may still turn out to start a
    /// sequence.
    pub fn pending_esc(&self) -> bool {
        self.pending == [ESC]
    }

    /// How many answers to [`STATUS_QUERY`] arrived since the last call.
    pub fn take_reports(&mut self) -> u32 {
        std::mem::take(&mut self.reports)
    }

    /// Nothing followed a lone ESC in time: it was the Esc key.
    pub fn flush(&mut self, out: &mut Vec<Key>) {
        if self.pending_esc() {
            out.push(Key::Esc);
        }
        self.pending.clear();
    }
}

enum Escape {
    /// The sequence's end hasn't arrived yet.
    Incomplete,
    /// A key (or nothing, for sequences the game ignores) and the bytes used.
    Key(Option<Key>, usize),
    /// The terminal's answer to [`STATUS_QUERY`], and the bytes used.
    Report(usize),
}

/// Parses an escape sequence at the start of `buf`, which begins with ESC.
fn parse_escape(buf: &[u8]) -> Escape {
    let arrow = |b: u8| match b {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        _ => None,
    };
    match buf.get(1) {
        None => Escape::Incomplete,
        // CSI: ESC [ <parameters> <final byte>. Arrows with modifiers (they
        // have parameters) are ignored, like other Ctrl/Alt combinations.
        Some(b'[') => match buf[2..].iter().position(|b| (0x40..=0x7e).contains(b)) {
            Some(end) => {
                let fin = buf[2 + end];
                // "0n" is all well; "3n", a terminal that's unwell, is an
                // answer too.
                if fin == b'n' && matches!(&buf[2..2 + end], b"0" | b"3") {
                    return Escape::Report(3 + end);
                }
                let key = if end == 0 { arrow(fin) } else { None };
                Escape::Key(key, 3 + end)
            }
            None if buf.len() > MAX_SEQUENCE => Escape::Key(None, buf.len()),
            None => Escape::Incomplete,
        },
        // SS3: ESC O <letter>, what arrows send in application cursor mode.
        Some(b'O') => match buf.get(2) {
            Some(&b) => Escape::Key(arrow(b), 3),
            None => Escape::Incomplete,
        },
        // ESC then an ordinary byte: Esc, and the byte is parsed on its own.
        Some(_) => Escape::Key(Some(Key::Esc), 1),
    }
}

/// Lets through at most `burst` at once, then `per_sec` a second: a token
/// bucket. Whatever a player sends past it is dropped, so a flood of keys
/// can't make the server do unbounded work.
pub struct Budget {
    burst: f64,
    per_sec: f64,
    left: f64,
    at: Instant,
}

impl Budget {
    pub fn new(burst: u32, per_sec: u32, now: Instant) -> Self {
        Self {
            burst: f64::from(burst),
            per_sec: f64::from(per_sec),
            left: f64::from(burst),
            at: now,
        }
    }

    /// How many of `wanted` may go ahead at `now`.
    pub fn take(&mut self, wanted: usize, now: Instant) -> usize {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        self.left = (self.left + elapsed * self.per_sec).min(self.burst);
        let allowed = (self.left.floor() as usize).min(wanted);
        self.left -= allowed as f64;
        allowed
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use Key::*;

    #[test]
    fn budgets_let_a_burst_through_then_a_steady_rate() {
        let start = Instant::now();
        let mut budget = Budget::new(256, 50, start);
        assert_eq!(budget.take(200, start), 200, "a pasted password");
        assert_eq!(budget.take(16_000, start), 56, "the rest of the burst");
        assert_eq!(budget.take(10, start), 0);
        let later = start + Duration::from_millis(100);
        assert_eq!(budget.take(10, later), 5, "50 a second");
        let much_later = later + Duration::from_secs(3600);
        assert_eq!(
            budget.take(1000, much_later),
            256,
            "never more than a burst"
        );
    }

    fn parse(chunks: &[&[u8]]) -> Vec<Key> {
        let mut parser = KeyParser::default();
        let mut keys = Vec::new();
        for chunk in chunks {
            parser.feed(chunk, &mut keys);
        }
        parser.flush(&mut keys);
        keys
    }

    #[test]
    fn arrows_in_both_forms() {
        assert_eq!(
            parse(&[b"\x1b[A\x1b[B\x1b[C\x1b[D"]),
            [Up, Down, Right, Left]
        );
        assert_eq!(parse(&[b"\x1bOA\x1bOD"]), [Up, Left]);
    }

    #[test]
    fn enter_ctrl_c_backspace_and_letters() {
        assert_eq!(parse(&[b"\r"]), [Enter]);
        assert_eq!(parse(&[b"\n"]), [Enter]);
        assert_eq!(parse(&[b"\r\n"]), [Enter], "CR LF is one Enter");
        assert_eq!(parse(&[b"\r", b"\n"]), [Enter], "even split across packets");
        assert_eq!(parse(&[b"\r\r"]), [Enter, Enter]);
        assert_eq!(
            parse(&[b"\x03\x7f\x08wD q"]),
            [
                CtrlC,
                Backspace,
                Backspace,
                Char('w'),
                Char('D'),
                Char(' '),
                Char('q')
            ]
        );
    }

    #[test]
    fn other_bytes_are_ignored() {
        // Ctrl+D, a tab, UTF-8 text, Ctrl+Up, an F-key.
        assert_eq!(parse(&[b"\x04\t\xc3\xa9\x1b[1;5A\x1b[15~"]), []);
    }

    #[test]
    fn a_lone_esc_waits_to_see_what_follows() {
        let mut parser = KeyParser::default();
        let mut keys = Vec::new();
        parser.feed(b"\x1b", &mut keys);
        assert!(keys.is_empty() && parser.pending_esc());
        parser.feed(b"[A", &mut keys);
        assert_eq!(keys, [Up], "it was the start of an arrow");

        keys.clear();
        parser.feed(b"\x1b", &mut keys);
        parser.flush(&mut keys);
        assert_eq!(keys, [Esc], "nothing followed: the Esc key");
        assert!(!parser.pending_esc());
    }

    #[test]
    fn esc_before_an_ordinary_key_is_esc_then_that_key() {
        assert_eq!(parse(&[b"\x1bq"]), [Esc, Char('q')]);
    }

    #[test]
    fn endless_sequences_are_dropped() {
        // Parameters that never reach a final byte.
        let mut junk = b"\x1b[".to_vec();
        junk.extend([b'1'; 40]);
        assert_eq!(parse(&[&junk, b"q"]), [Char('q')]);
    }

    #[test]
    fn status_reports_are_counted_and_never_keys() {
        let mut parser = KeyParser::default();
        let mut keys = Vec::new();
        parser.feed(b"\x1b[A\x1b[0nq\x1b[3n", &mut keys);
        assert_eq!(keys, [Up, Char('q')]);
        assert_eq!(parser.take_reports(), 2);
        assert_eq!(parser.take_reports(), 0, "taken");

        // Split anywhere, still one answer and no Esc.
        for at in 1..4 {
            let (a, b) = b"\x1b[0n".split_at(at);
            parser.feed(a, &mut keys);
            parser.feed(b, &mut keys);
            parser.flush(&mut keys);
            assert_eq!(keys, [Up, Char('q')], "split at {at}");
            assert_eq!(parser.take_reports(), 1, "split at {at}");
        }
        // Other status-like sequences are ignored, not counted.
        parser.feed(b"\x1b[6n\x1b[1n", &mut keys);
        assert_eq!(parser.take_reports(), 0);
    }

    #[test]
    fn any_split_gives_the_same_keys() {
        let input: &[u8] = b"w\x1b[A\x1bOB\r\n\x03\x1b[1;2C\x7fq\x1bx";
        let whole = parse(&[input]);
        assert_eq!(
            whole,
            [
                Char('w'),
                Up,
                Down,
                Enter,
                CtrlC,
                Backspace,
                Char('q'),
                Esc,
                Char('x')
            ]
        );
        for a in 0..=input.len() {
            for b in a..=input.len() {
                let split = parse(&[&input[..a], &input[a..b], &input[b..]]);
                assert_eq!(split, whole, "split at {a} and {b}");
            }
        }
    }
}
