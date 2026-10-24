//! Who a connection belongs to.

use russh::keys::{HashAlg, PublicKey};

/// Longest nickname shown to other players.
pub const MAX_NICK: usize = 16;

/// User names that say nothing about the person: whoever typed
/// `ssh root@…` or kept their machine's login is given a generated name.
const GENERIC_USERS: &[&str] = &[
    "root",
    "admin",
    "administrator",
    "user",
    "guest",
    "ubuntu",
    "debian",
    "centos",
    "ec2-user",
    "pi",
    "test",
    "git",
    "play",
    "snake",
    "snaketui",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    /// Connected with this SSH key (its SHA-256 fingerprint). Settings and
    /// records are kept for them between visits.
    Key(String),
    /// No key: nothing is kept.
    Guest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub identity: Identity,
    pub nick: String,
    /// Four hex characters, for names that would otherwise say nothing.
    pub tag: String,
}

impl Player {
    pub fn with_key(ssh_user: &str, key: &PublicKey) -> Self {
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let tag = tag_from(&fingerprint);
        Self {
            nick: nickname(ssh_user, &tag),
            identity: Identity::Key(fingerprint),
            tag,
        }
    }

    pub fn guest(ssh_user: &str) -> Self {
        let tag = format!("{:04x}", rand::random::<u16>());
        Self {
            nick: nickname(ssh_user, &tag),
            identity: Identity::Guest,
            tag,
        }
    }

    /// How other players see this one. Guests' names start with `~`, which
    /// no account name will be able to.
    pub fn display_name(&self) -> String {
        format!("~{}", self.nick)
    }

    pub fn fingerprint(&self) -> Option<&str> {
        match &self.identity {
            Identity::Key(fp) => Some(fp),
            Identity::Guest => None,
        }
    }
}

/// Four hex characters derived from a fingerprint, stable per key.
fn tag_from(fingerprint: &str) -> String {
    let hash = fingerprint
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    format!("{:04x}", hash & 0xffff)
}

/// The SSH user name as a nickname that's safe to draw on other players'
/// screens: only `A-Z a-z 0-9 _ -`, at most [`MAX_NICK`] characters, so it
/// can't carry terminal escape sequences. Empty or generic names become
/// `player-<tag>`.
pub fn nickname(ssh_user: &str, tag: &str) -> String {
    let clean: String = ssh_user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(MAX_NICK)
        .collect();
    if clean.is_empty() || GENERIC_USERS.contains(&clean.to_ascii_lowercase().as_str()) {
        format!("player-{tag}")
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nicknames_are_safe_to_show() {
        assert_eq!(nickname("alice", "0a1b"), "alice");
        assert_eq!(nickname("\x1b[31mevil\x1b[0m", "0a1b"), "31mevil0m");
        assert_eq!(
            nickname("a.very-long_name.with.dots!", "0a1b"),
            "avery-long_namew"
        );
        assert_eq!(nickname("Root", "0a1b"), "player-0a1b");
        assert_eq!(nickname("ubuntu", "0a1b"), "player-0a1b");
        assert_eq!(nickname("", "0a1b"), "player-0a1b");
        assert_eq!(nickname("日本", "0a1b"), "player-0a1b");
    }

    #[test]
    fn tags_are_stable_per_key() {
        assert_eq!(tag_from("SHA256:abc"), tag_from("SHA256:abc"));
        assert_ne!(tag_from("SHA256:abc"), tag_from("SHA256:abd"));
    }
}
