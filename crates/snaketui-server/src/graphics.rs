//! Pictures in a player's terminal, for terminals that show them: by
//! iTerm2's inline images or by Kitty's graphics. Each frame says which
//! picture shows where; this sends what changed since the last.

use std::collections::HashMap;
use std::io::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ratatui::layout::Rect;
use shell_tui::Placement;

/// Asks a terminal what it is, whether it shows Kitty's pictures and how
/// big its cells are in pixels; then for its device attributes, which
/// every terminal answers once it has answered the rest.
pub const PROBE: &[u8] = b"\x1b[>0q\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c";

/// Kitty takes a picture in pieces of at most this much base64.
const CHUNK: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// `OSC 1337 File=`: iTerm2's, which WezTerm shows too.
    Iterm,
    /// `APC G`: Kitty's, which Ghostty and others show too.
    Kitty,
}

impl Protocol {
    /// How a terminal that answered with `version`, and said yes to Kitty's
    /// query or not, shows pictures; `None` if it doesn't.
    pub fn of(version: Option<&str>, kitty: bool) -> Option<Protocol> {
        let name = version.unwrap_or_default();
        if name.starts_with("iTerm2") || name.starts_with("WezTerm") {
            Some(Protocol::Iterm)
        } else if kitty {
            Some(Protocol::Kitty)
        } else {
            None
        }
    }
}

/// A picture shown: which, and for Kitty, its id and the placement's.
struct Shown {
    picture: u64,
    kitty: Option<(u32, u32)>,
}

/// What a player's terminal shows of the pictures, and how to change it.
pub struct Graphics {
    protocol: Protocol,
    shown: HashMap<Rect, Shown>,
    /// Kitty: the pictures it has been sent, by their id here, and its id
    /// for each.
    sent: HashMap<u64, u32>,
    /// Kitty: the last id given to a picture or placement.
    last_id: u32,
}

impl Graphics {
    pub fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            shown: HashMap::new(),
            sent: HashMap::new(),
            last_id: 0,
        }
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    fn next_id(&mut self) -> u32 {
        self.last_id = self.last_id.wrapping_add(1).max(1);
        self.last_id
    }

    /// Changes what shows to `placements`, after the frame's text in
    /// `out`. Text has been drawn wherever a picture went; iTerm2 shows
    /// that, but Kitty's pictures stay until they're deleted.
    pub fn frame(&mut self, placements: &[Placement], out: &mut Vec<u8>) {
        let now: HashMap<Rect, u64> = placements.iter().map(|p| (p.area, p.picture.id)).collect();
        let mut bytes = Vec::new();
        self.shown.retain(|area, shown| {
            let stays = now.get(area) == Some(&shown.picture);
            if !stays && let Some((image, placement)) = shown.kitty {
                let _ = write!(bytes, "\x1b_Ga=d,d=i,i={image},p={placement},q=2\x1b\\");
            }
            stays
        });
        for p in placements {
            if self.shown.contains_key(&p.area) {
                continue;
            }
            let Rect {
                x,
                y,
                width,
                height,
            } = p.area;
            let _ = write!(bytes, "\x1b[{};{}H", y + 1, x + 1);
            let kitty = match self.protocol {
                Protocol::Iterm => {
                    let _ = write!(
                        bytes,
                        "\x1b]1337;File=inline=1;size={};width={width};height={height};\
                         preserveAspectRatio=0;doNotMoveCursor=1:{}\x07",
                        p.picture.png.len(),
                        STANDARD.encode(&p.picture.png),
                    );
                    None
                }
                Protocol::Kitty => {
                    let image = match self.sent.get(&p.picture.id) {
                        Some(&image) => image,
                        None => {
                            let image = self.next_id();
                            upload(image, &p.picture.png, &mut bytes);
                            self.sent.insert(p.picture.id, image);
                            image
                        }
                    };
                    let placement = self.next_id();
                    let _ = write!(
                        bytes,
                        "\x1b_Ga=p,i={image},p={placement},c={width},r={height},C=1,q=2\x1b\\"
                    );
                    Some((image, placement))
                }
            };
            let shown = Shown {
                picture: p.picture.id,
                kitty,
            };
            self.shown.insert(p.area, shown);
        }
        if !bytes.is_empty() {
            // Where the frame left the cursor, it stays.
            out.extend_from_slice(b"\x1b7");
            out.extend(bytes);
            out.extend_from_slice(b"\x1b8");
        }
    }

    /// The screen is being cleared, as on a resize: every picture is drawn
    /// again with the next frame.
    pub fn reset(&mut self) -> Vec<u8> {
        self.shown.clear();
        match self.protocol {
            Protocol::Iterm => Vec::new(),
            Protocol::Kitty => b"\x1b_Ga=d,d=a,q=2\x1b\\".to_vec(),
        }
    }

    /// The player is leaving: their terminal can forget every picture.
    pub fn leave(&self) -> &'static [u8] {
        match self.protocol {
            Protocol::Iterm => b"",
            Protocol::Kitty => b"\x1b_Ga=d,d=A,q=2\x1b\\",
        }
    }
}

/// Sends Kitty `png` as picture `image`, in pieces.
fn upload(image: u32, png: &[u8], out: &mut Vec<u8>) {
    let data = STANDARD.encode(png);
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(CHUNK).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let chunk = std::str::from_utf8(chunk).expect("base64 is ASCII");
        if i == 0 {
            let _ = write!(out, "\x1b_Ga=t,f=100,i={image},q=2,m={more};{chunk}\x1b\\");
        } else {
            let _ = write!(out, "\x1b_Gm={more};{chunk}\x1b\\");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ratatui::buffer::Buffer;
    use shell_tui::{Picture, Pictures};

    use super::*;

    /// `pictures` placed, each as (x, id, PNG bytes), on a 40×4 screen.
    fn placed(pictures: &[(u16, u64, usize)]) -> Vec<Placement> {
        let buf = Buffer::empty(Rect::new(0, 0, 40, 4));
        let frame = Pictures::new((8, 16));
        for &(x, id, len) in pictures {
            let picture = Picture {
                id,
                png: Arc::from(vec![7; len]),
            };
            frame.place(&buf, Rect::new(x, 1, 5, 2), picture);
        }
        frame.take()
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn terminals_are_told_apart_by_what_they_answer() {
        assert_eq!(
            Protocol::of(Some("iTerm2 3.7.5"), true),
            Some(Protocol::Iterm)
        );
        assert_eq!(
            Protocol::of(Some("WezTerm 2024"), false),
            Some(Protocol::Iterm)
        );
        assert_eq!(
            Protocol::of(Some("kitty(0.40)"), true),
            Some(Protocol::Kitty)
        );
        assert_eq!(Protocol::of(None, true), Some(Protocol::Kitty));
        assert_eq!(Protocol::of(Some("tmux 3.5"), false), None);
        assert_eq!(Protocol::of(None, false), None);
    }

    #[test]
    fn iterm2_is_sent_what_changed_and_nothing_else() {
        let mut g = Graphics::new(Protocol::Iterm);
        let mut out = Vec::new();
        g.frame(&placed(&[(0, 1, 3), (5, 2, 3)]), &mut out);
        let first = text(&out);
        assert!(first.starts_with("\x1b7") && first.ends_with("\x1b8"));
        assert_eq!(
            first
                .matches("\x1b]1337;File=inline=1;size=3;width=5;height=2;")
                .count(),
            2
        );
        assert!(first.contains("\x1b[2;1H") && first.contains("\x1b[2;6H"));
        assert!(first.contains(":BwcH\x07"), "the PNG, in base64");

        out.clear();
        g.frame(&placed(&[(0, 1, 3), (5, 2, 3)]), &mut out);
        assert!(out.is_empty(), "nothing changed");
        // A piece taken: a new picture where the old one was; one gone.
        g.frame(&placed(&[(5, 3, 3)]), &mut out);
        assert_eq!(text(&out).matches("File=").count(), 1);
        assert!(text(&out).contains("\x1b[2;6H"));
        out.clear();
        assert!(g.reset().is_empty());
        g.frame(&placed(&[(5, 3, 3)]), &mut out);
        assert_eq!(
            text(&out).matches("File=").count(),
            1,
            "drawn again after a clear"
        );
    }

    #[test]
    fn kitty_gets_each_picture_once_and_its_placements_deleted() {
        let mut g = Graphics::new(Protocol::Kitty);
        let mut out = Vec::new();
        // The same picture twice, and one big enough to go in pieces.
        g.frame(&placed(&[(0, 1, 3), (5, 1, 3), (10, 2, 4000)]), &mut out);
        let first = text(&out);
        assert_eq!(first.matches("a=t,f=100").count(), 2, "{first}");
        assert_eq!(first.matches("a=p,").count(), 3);
        assert!(first.contains("c=5,r=2,C=1,q=2"));
        assert!(first.contains(",m=1;") && first.contains("\x1b_Gm=0;"));

        out.clear();
        g.frame(&placed(&[(0, 1, 3), (10, 2, 4000)]), &mut out);
        let next = text(&out);
        assert_eq!(next.matches("a=d,d=i,").count(), 1, "the one gone: {next}");
        assert!(!next.contains("a=t") && !next.contains("a=p"));

        out.clear();
        g.frame(&placed(&[(0, 2, 4000), (10, 2, 4000)]), &mut out);
        let swapped = text(&out);
        assert_eq!(swapped.matches("a=d,d=i,").count(), 1);
        assert_eq!(swapped.matches("a=p,").count(), 1);
        assert!(!swapped.contains("a=t"), "sent already");
        assert_eq!(g.reset(), b"\x1b_Ga=d,d=a,q=2\x1b\\");
        assert_eq!(g.leave(), b"\x1b_Ga=d,d=A,q=2\x1b\\");
    }
}
