//! Turns the bytes a player's terminal sends into [`Key`]s and the
//! terminal's own [`Answer`]s, and keeps how fast anyone sends them within
//! reason.

use std::time::Instant;

use shell_tui::{Key, Mouse, MouseKind};

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// A sequence longer than this is garbage, not a key.
const MAX_SEQUENCE: usize = 32;
/// An answer longer than this is garbage too.
const MAX_ANSWER: usize = 256;
/// Asks the terminal how it is (device status report). Every terminal since
/// the VT100 answers at once with `ESC [ 0 n`, which no key sends, so the
/// wait for the answer is the player's round trip.
pub const STATUS_QUERY: &[u8] = b"\x1b[5n";

/// What a terminal says about itself when asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Its name and version (XTVERSION), such as "iTerm2 3.7.5".
    Version(String),
    /// Whether it shows pictures sent Kitty's way.
    Kitty(bool),
    /// A cell's size in pixels, width then height.
    CellSize(u16, u16),
    /// Its device attributes (DA1). Every terminal answers, in order, so
    /// this comes after the answers to whatever was asked before it.
    Attributes,
}

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
    /// Other answers since [`Self::take_answers`].
    answers: Vec<Answer>,
    /// Answers that come as strings are on their way: `ESC P`, `ESC _` and
    /// `ESC ]` start one, not Alt with a key.
    expecting: bool,
}

impl KeyParser {
    /// Parses `bytes`, appending the keys they contain to `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Key>) {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(bytes);
        let mut i = 0;
        while i < buf.len() {
            if buf[i] == ESC {
                match parse_escape(&buf[i..], self.expecting) {
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
                    Escape::Answer(answer, len) => {
                        self.answers.push(answer);
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

    /// The terminal's other answers since the last call.
    pub fn take_answers(&mut self) -> Vec<Answer> {
        std::mem::take(&mut self.answers)
    }

    /// Whether answers that come as strings are on their way: until they
    /// have come, Alt with `P`, `_` or `]` isn't read as such.
    pub fn expect_answers(&mut self, expecting: bool) {
        self.expecting = expecting;
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
    /// Another answer of the terminal's, and the bytes used.
    Answer(Answer, usize),
}

/// A mouse report's `b;x;y`, with `M` for a press or `m` for a release:
/// the left button only, at a 0-based cell.
fn mouse(params: &[u8], fin: u8) -> Option<Key> {
    let text = std::str::from_utf8(params).ok()?;
    let mut parts = text.split(';').map(|p| p.parse::<u16>().ok());
    let (button, x, y) = (parts.next()??, parts.next()??, parts.next()??);
    // Shift, Alt and Ctrl don't matter; motion, the wheel and the other
    // buttons aren't used.
    if button & !(4 | 8 | 16) != 0 || x == 0 || y == 0 {
        return None;
    }
    let kind = if fin == b'M' {
        MouseKind::Press
    } else {
        MouseKind::Release
    };
    Some(Key::Mouse(Mouse {
        kind,
        x: x - 1,
        y: y - 1,
    }))
}

/// An answer that came as a string: XTVERSION's in DCS (`>|` then the
/// name), or Kitty's in APC (`G`, its keys, then `;OK` or why not).
fn string_answer(intro: u8, body: &[u8]) -> Option<Answer> {
    match intro {
        b'P' => {
            let name = body.strip_prefix(b">|")?;
            Some(Answer::Version(String::from_utf8_lossy(name).into_owned()))
        }
        b'_' => {
            let reply = body.strip_prefix(b"G")?;
            let message = reply
                .iter()
                .position(|&b| b == b';')
                .map(|i| &reply[i + 1..]);
            Some(Answer::Kitty(message == Some(b"OK")))
        }
        _ => None,
    }
}

/// Parses an escape sequence at the start of `buf`, which begins with ESC.
/// With `strings`, `ESC P`, `ESC _` and `ESC ]` start answers.
fn parse_escape(buf: &[u8], strings: bool) -> Escape {
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
                let params = &buf[2..2 + end];
                let len = 3 + end;
                // "0n" is all well; "3n", a terminal that's unwell, is an
                // answer too.
                if fin == b'n' && matches!(params, b"0" | b"3") {
                    return Escape::Report(len);
                }
                match (params.first(), fin) {
                    (Some(b'<'), b'M' | b'm') => return Escape::Key(mouse(&params[1..], fin), len),
                    (Some(b'?'), b'c') => return Escape::Answer(Answer::Attributes, len),
                    (Some(b'6'), b't') => {
                        let text = std::str::from_utf8(params).unwrap_or("");
                        let sizes: Vec<u16> =
                            text.split(';').filter_map(|p| p.parse().ok()).collect();
                        if let [6, h, w] = sizes[..] {
                            return Escape::Answer(Answer::CellSize(w, h), len);
                        }
                    }
                    _ => {}
                }
                let key = if end == 0 { arrow(fin) } else { None };
                Escape::Key(key, len)
            }
            None if buf.len() > MAX_SEQUENCE => Escape::Key(None, buf.len()),
            None => Escape::Incomplete,
        },
        // A string, ended by ST (`ESC \`) or BEL: an answer, while some are
        // on their way.
        Some(&intro @ (b'P' | b'_' | b']')) if strings => {
            let body = &buf[2..];
            let end = body.iter().enumerate().find_map(|(i, &b)| match b {
                BEL => Some((i, 1)),
                ESC if body.get(i + 1) == Some(&b'\\') => Some((i, 2)),
                _ => None,
            });
            match end {
                Some((i, term)) => match string_answer(intro, &body[..i]) {
                    Some(answer) => Escape::Answer(answer, 2 + i + term),
                    None => Escape::Key(None, 2 + i + term),
                },
                None if buf.len() > MAX_ANSWER => Escape::Key(None, buf.len()),
                None => Escape::Incomplete,
            }
        }
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

    /// What `chunks` hold, with answers expected: keys, then answers.
    fn answered(chunks: &[&[u8]]) -> (Vec<Key>, Vec<Answer>) {
        let mut parser = KeyParser::default();
        parser.expect_answers(true);
        let mut keys = Vec::new();
        for chunk in chunks {
            parser.feed(chunk, &mut keys);
        }
        parser.flush(&mut keys);
        (keys, parser.take_answers())
    }

    #[test]
    fn a_terminals_answers_are_read_and_never_keys() {
        let input: &[u8] = b"\x1bP>|iTerm2 3.7.5\x1b\\\x1b_Gi=31;OK\x1b\\w\x1b[6;16;8t\
            \x1b]11;rgb:0/0/0\x07\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\\x1b[?62;4cq";
        let (keys, answers) = answered(&[input]);
        assert_eq!(keys, [Char('w'), Char('q')]);
        assert_eq!(
            answers,
            [
                Answer::Version("iTerm2 3.7.5".into()),
                Answer::Kitty(true),
                Answer::CellSize(8, 16),
                Answer::Kitty(false),
                Answer::Attributes,
            ]
        );
        for at in 0..=input.len() {
            let split = answered(&[&input[..at], &input[at..]]);
            assert_eq!(split, (keys.clone(), answers.clone()), "split at {at}");
        }
        // Too long to be an answer: dropped, and what follows is read.
        let mut parser = KeyParser::default();
        parser.expect_answers(true);
        let mut keys = Vec::new();
        let mut junk = b"\x1bP".to_vec();
        junk.extend([b'x'; 300]);
        parser.feed(&junk, &mut keys);
        parser.feed(b"w", &mut keys);
        assert_eq!(keys, [Char('w')]);
        assert!(parser.take_answers().is_empty());
    }

    #[test]
    fn alt_with_a_letter_is_esc_and_the_letter_unless_answers_are_due() {
        assert_eq!(
            parse(&[b"\x1bPq\x1b_"]),
            [Esc, Char('P'), Char('q'), Esc, Char('_')]
        );
        // Even due, DA1 always answers in the end.
        assert_eq!(answered(&[b"\x1b[?1;2c"]).1, [Answer::Attributes]);
    }

    #[test]
    fn the_left_button_comes_as_presses_and_releases() {
        let at = |kind, x, y| Mouse(shell_tui::Mouse { kind, x, y });
        // A press and release, a drag, the wheel, the right button, and a
        // press with Shift.
        let input = b"\x1b[<0;5;3M\x1b[<0;6;3m\x1b[<32;6;3M\x1b[<64;1;1M\x1b[<2;1;1M\x1b[<4;2;2M";
        assert_eq!(
            parse(&[input]),
            [
                at(MouseKind::Press, 4, 2),
                at(MouseKind::Release, 5, 2),
                at(MouseKind::Press, 1, 1),
            ]
        );
        assert_eq!(
            parse(&[b"\x1b[<0;0;3M\x1b[<0;2M"]),
            [],
            "nonsense is dropped"
        );
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
