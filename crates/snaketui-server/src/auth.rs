//! Signing up and signing in: what usernames and passwords may be, hashing
//! passwords, and how many wrong guesses anyone gets.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use crate::identity::GENERIC_USERS;

pub const MIN_USERNAME: usize = 3;
pub const MAX_USERNAME: usize = 16;
pub const MIN_PASSWORD: usize = 8;
pub const MAX_PASSWORD: usize = 128;

/// Names nobody may take, on top of the generic login names.
const RESERVED: &[&str] = &[
    "anonymous",
    "everyone",
    "mod",
    "moderator",
    "null",
    "official",
    "server",
    "staff",
    "support",
    "system",
];

/// Why a username can't be used.
pub fn check_username(name: &str) -> Result<(), &'static str> {
    if !(MIN_USERNAME..=MAX_USERNAME).contains(&name.len()) {
        return Err("Usernames are 3 to 16 characters.");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("Usernames use letters, digits, _ and - only.");
    }
    let lower = name.to_ascii_lowercase();
    if GENERIC_USERS.contains(&lower.as_str()) || RESERVED.contains(&lower.as_str()) {
        return Err("That username is reserved. Pick another.");
    }
    Ok(())
}

/// Why a password can't be used.
pub fn check_password(username: &str, password: &str) -> Result<(), &'static str> {
    let len = password.chars().count();
    if len < MIN_PASSWORD {
        return Err("Passwords are at least 8 characters.");
    }
    if len > MAX_PASSWORD {
        return Err("Passwords are at most 128 characters.");
    }
    if password.eq_ignore_ascii_case(username) {
        return Err("Your password can't be your username.");
    }
    Ok(())
}

/// Argon2id with OWASP's settings (19 MiB, 2 passes), on blocking threads,
/// a few at a time, so a flood of sign-ins can't use up memory or CPU.
#[derive(Clone)]
pub struct Hasher {
    permits: Arc<Semaphore>,
    /// Checked when a username doesn't exist, so a wrong name takes as long
    /// as a wrong password.
    decoy: Arc<str>,
}

/// Password checks running at once.
const AT_ONCE: usize = 4;

impl Hasher {
    pub fn new() -> Result<Self> {
        let decoy = Argon2::default()
            .hash_password(b"not anyone's password")
            .map_err(|e| anyhow!("hashing: {e}"))?
            .to_string();
        Ok(Self {
            permits: Arc::new(Semaphore::new(AT_ONCE)),
            decoy: decoy.into(),
        })
    }

    pub async fn hash(&self, password: Zeroizing<String>) -> Result<String> {
        let _permit = self.permits.acquire().await?;
        tokio::task::spawn_blocking(move || {
            Argon2::default()
                .hash_password(password.as_bytes())
                .map(|h| h.to_string())
                .map_err(|e| anyhow!("hashing: {e}"))
        })
        .await?
    }

    /// Whether `password` matches `hash`. With no hash, checks the decoy and
    /// says no.
    pub async fn verify(&self, password: Zeroizing<String>, hash: Option<String>) -> bool {
        let Ok(_permit) = self.permits.acquire().await else {
            return false;
        };
        let real = hash.is_some();
        let hash = hash.unwrap_or_else(|| self.decoy.to_string());
        let checked = tokio::task::spawn_blocking(move || {
            PasswordHash::new(&hash).is_ok_and(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
        })
        .await;
        real && checked.unwrap_or(false)
    }
}

/// Wrong guesses allowed for one username before it's locked.
pub const NAME_TRIES: usize = 5;
pub const NAME_WINDOW: Duration = Duration::from_secs(15 * 60);
pub const NAME_LOCK: Duration = Duration::from_secs(15 * 60);
/// Wrong guesses allowed from one IP address.
pub const IP_TRIES: usize = 20;
pub const IP_WINDOW: Duration = Duration::from_secs(60 * 60);
/// Accounts one IP address may make.
pub const SIGNUPS: usize = 5;
pub const SIGNUP_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// This username had too many wrong guesses; it's locked for a while.
    NameLocked,
    /// This address made too many wrong guesses.
    TooManyFromAddress,
    /// This address made too many accounts today.
    TooManySignUps,
}

impl Refused {
    pub fn message(self) -> &'static str {
        match self {
            Refused::NameLocked => "Too many wrong tries for that name. Try again in 15 minutes.",
            Refused::TooManyFromAddress => {
                "Too many wrong tries from your address. Try again later."
            }
            Refused::TooManySignUps => "Too many new accounts from your address today.",
        }
    }
}

/// Who has guessed wrong, and who has made accounts, recently. Times are
/// passed in, so tests drive the clock.
#[derive(Default)]
pub struct Attempts {
    /// By lowercase username.
    name_failures: HashMap<String, Vec<Instant>>,
    locked_until: HashMap<String, Instant>,
    address_failures: HashMap<IpAddr, Vec<Instant>>,
    signups: HashMap<IpAddr, Vec<Instant>>,
}

fn recent(times: &mut Vec<Instant>, window: Duration, now: Instant) -> usize {
    times.retain(|t| now.saturating_duration_since(*t) < window);
    times.len()
}

impl Attempts {
    pub fn may_sign_in(&mut self, name: &str, from: IpAddr, now: Instant) -> Result<(), Refused> {
        let name = name.to_ascii_lowercase();
        if self
            .locked_until
            .get(&name)
            .is_some_and(|until| now < *until)
        {
            return Err(Refused::NameLocked);
        }
        let tries = self.address_failures.entry(from).or_default();
        if recent(tries, IP_WINDOW, now) >= IP_TRIES {
            return Err(Refused::TooManyFromAddress);
        }
        Ok(())
    }

    pub fn failed(&mut self, name: &str, from: IpAddr, now: Instant) {
        let name = name.to_ascii_lowercase();
        let tries = self.name_failures.entry(name.clone()).or_default();
        tries.push(now);
        if recent(tries, NAME_WINDOW, now) >= NAME_TRIES {
            tries.clear();
            self.locked_until.insert(name, now + NAME_LOCK);
        }
        self.address_failures.entry(from).or_default().push(now);
    }

    pub fn succeeded(&mut self, name: &str) {
        self.name_failures.remove(&name.to_ascii_lowercase());
    }

    pub fn may_sign_up(&mut self, from: IpAddr, now: Instant) -> Result<(), Refused> {
        let made = self.signups.entry(from).or_default();
        if recent(made, SIGNUP_WINDOW, now) >= SIGNUPS {
            return Err(Refused::TooManySignUps);
        }
        Ok(())
    }

    pub fn signed_up(&mut self, from: IpAddr, now: Instant) {
        self.signups.entry(from).or_default().push(now);
    }

    /// Drops what no longer matters, so the maps don't grow forever.
    pub fn tidy(&mut self, now: Instant) {
        self.name_failures
            .retain(|_, t| recent(t, NAME_WINDOW, now) > 0);
        self.locked_until.retain(|_, until| now < *until);
        self.address_failures
            .retain(|_, t| recent(t, IP_WINDOW, now) > 0);
        self.signups
            .retain(|_, t| recent(t, SIGNUP_WINDOW, now) > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_short_plain_and_not_reserved() {
        assert!(check_username("alice").is_ok());
        assert!(check_username("Al_ice-9").is_ok());
        assert!(check_username("al").is_err());
        assert!(check_username("a-very-long-username").is_err());
        assert!(check_username("ali ce").is_err());
        assert!(check_username("~alice").is_err(), "guests' mark");
        assert!(check_username("Root").is_err());
        assert!(check_username("ADMIN").is_err());
        assert!(check_username("moderator").is_err());
    }

    #[test]
    fn passwords_are_long_enough_and_not_the_username() {
        assert!(check_password("alice", "correct horse").is_ok());
        assert!(check_password("alice", "short").is_err());
        assert!(check_password("alice123", "ALICE123").is_err());
        assert!(check_password("alice", &"x".repeat(129)).is_err());
    }

    #[actix_rt::test]
    async fn hashing_and_checking_passwords() {
        let hasher = Hasher::new().unwrap();
        let secret = |s: &str| Zeroizing::new(s.to_string());
        let hash = hasher.hash(secret("correct horse")).await.unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(!hash.contains("correct horse"));
        assert!(
            hasher
                .verify(secret("correct horse"), Some(hash.clone()))
                .await
        );
        assert!(!hasher.verify(secret("wrong horse"), Some(hash)).await);
        assert!(!hasher.verify(secret("not anyone's password"), None).await);
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn five_wrong_guesses_lock_a_name_for_a_while() {
        let mut attempts = Attempts::default();
        let start = Instant::now();
        for i in 0..NAME_TRIES {
            assert!(attempts.may_sign_in("Alice", ip(i as u8), start).is_ok());
            attempts.failed("Alice", ip(i as u8), start);
        }
        assert_eq!(
            attempts.may_sign_in("alice", ip(99), start),
            Err(Refused::NameLocked),
            "from anywhere, in any case"
        );
        assert!(attempts.may_sign_in("bob", ip(99), start).is_ok());
        let later = start + NAME_LOCK;
        assert!(attempts.may_sign_in("alice", ip(99), later).is_ok());
    }

    #[test]
    fn guesses_spread_out_in_time_never_lock() {
        let mut attempts = Attempts::default();
        let mut now = Instant::now();
        for _ in 0..20 {
            attempts.failed("alice", ip(1), now);
            attempts.succeeded("alice");
            now += Duration::from_secs(60);
        }
        assert!(attempts.may_sign_in("alice", ip(2), now).is_ok());
        for _ in 0..NAME_TRIES - 1 {
            attempts.failed("alice", ip(2), now);
            now += NAME_WINDOW / 4;
        }
        assert!(attempts.may_sign_in("alice", ip(2), now).is_ok());
    }

    #[test]
    fn one_address_gets_twenty_wrong_guesses_an_hour() {
        let mut attempts = Attempts::default();
        let now = Instant::now();
        for i in 0..IP_TRIES {
            attempts.failed(&format!("user{i}"), ip(1), now);
        }
        assert_eq!(
            attempts.may_sign_in("someone", ip(1), now),
            Err(Refused::TooManyFromAddress)
        );
        assert!(attempts.may_sign_in("someone", ip(2), now).is_ok());
        assert!(
            attempts
                .may_sign_in("someone", ip(1), now + IP_WINDOW)
                .is_ok()
        );
    }

    #[test]
    fn one_address_makes_five_accounts_a_day() {
        let mut attempts = Attempts::default();
        let now = Instant::now();
        for _ in 0..SIGNUPS {
            assert!(attempts.may_sign_up(ip(1), now).is_ok());
            attempts.signed_up(ip(1), now);
        }
        assert_eq!(
            attempts.may_sign_up(ip(1), now),
            Err(Refused::TooManySignUps)
        );
        attempts.tidy(now + SIGNUP_WINDOW);
        assert!(attempts.may_sign_up(ip(1), now + SIGNUP_WINDOW).is_ok());
    }
}
