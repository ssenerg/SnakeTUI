//! Signing up and signing in: what usernames and passwords may be, hashing
//! passwords, and how many wrong guesses anyone gets.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use crate::identity::GENERIC_USERS;
use crate::store::{Counted, Store};

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

/// Who has guessed wrong, and who has made accounts, recently. The counts
/// live in the store, so every server sharing a database sees the same
/// ones. Times are seconds since the epoch, passed in so tests can drive
/// the clock.
#[derive(Clone)]
pub struct GuessLimits {
    store: Store,
}

fn secs(d: Duration) -> i64 {
    d.as_secs() as i64
}

impl GuessLimits {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn may_sign_in(
        &self,
        name: &str,
        from: IpAddr,
        now: i64,
    ) -> Result<Result<(), Refused>> {
        let name = name.to_ascii_lowercase();
        if self
            .store
            .name_locked_until(&name)
            .await?
            .is_some_and(|until| now < until)
        {
            return Ok(Err(Refused::NameLocked));
        }
        let tries = self
            .store
            .count(
                Counted::WrongFromAddress,
                &from.to_string(),
                now - secs(IP_WINDOW),
            )
            .await?;
        if tries >= IP_TRIES as i64 {
            return Ok(Err(Refused::TooManyFromAddress));
        }
        Ok(Ok(()))
    }

    pub async fn failed(&self, name: &str, from: IpAddr, now: i64) -> Result<()> {
        let name = name.to_ascii_lowercase();
        let store = &self.store;
        store
            .note(Counted::WrongFromAddress, &from.to_string(), now)
            .await?;
        store.note(Counted::WrongForName, &name, now).await?;
        let tries = store
            .count(Counted::WrongForName, &name, now - secs(NAME_WINDOW))
            .await?;
        if tries >= NAME_TRIES as i64 {
            store.forget(Counted::WrongForName, &name).await?;
            store.lock_name(&name, now + secs(NAME_LOCK)).await?;
        }
        Ok(())
    }

    pub async fn succeeded(&self, name: &str) -> Result<()> {
        self.store
            .forget(Counted::WrongForName, &name.to_ascii_lowercase())
            .await
    }

    pub async fn may_sign_up(&self, from: IpAddr, now: i64) -> Result<Result<(), Refused>> {
        let made = self
            .store
            .count(
                Counted::SignUp,
                &from.to_string(),
                now - secs(SIGNUP_WINDOW),
            )
            .await?;
        Ok(if made >= SIGNUPS as i64 {
            Err(Refused::TooManySignUps)
        } else {
            Ok(())
        })
    }

    pub async fn signed_up(&self, from: IpAddr, now: i64) -> Result<()> {
        self.store
            .note(Counted::SignUp, &from.to_string(), now)
            .await
    }

    /// Forgets what no longer counts, so the tables stay small.
    pub async fn tidy(&self, now: i64) -> Result<()> {
        let longest = NAME_WINDOW.max(IP_WINDOW).max(SIGNUP_WINDOW);
        self.store.tidy_limits(now - secs(longest), now).await
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

    async fn limits() -> GuessLimits {
        GuessLimits::new(Store::open("sqlite::memory:").await.unwrap())
    }

    const START: i64 = 1_000_000;

    #[actix_rt::test]
    async fn five_wrong_guesses_lock_a_name_for_a_while() {
        let limits = limits().await;
        for i in 0..NAME_TRIES {
            let from = ip(i as u8);
            assert_eq!(
                limits.may_sign_in("Alice", from, START).await.unwrap(),
                Ok(())
            );
            limits.failed("Alice", from, START).await.unwrap();
        }
        assert_eq!(
            limits.may_sign_in("alice", ip(99), START).await.unwrap(),
            Err(Refused::NameLocked),
            "from anywhere, in any case"
        );
        assert_eq!(
            limits.may_sign_in("bob", ip(99), START).await.unwrap(),
            Ok(())
        );
        let later = START + secs(NAME_LOCK);
        assert_eq!(
            limits.may_sign_in("alice", ip(99), later).await.unwrap(),
            Ok(())
        );
    }

    #[actix_rt::test]
    async fn guesses_spread_out_in_time_never_lock() {
        let limits = limits().await;
        let mut now = START;
        for _ in 0..20 {
            limits.failed("alice", ip(1), now).await.unwrap();
            limits.succeeded("alice").await.unwrap();
            now += 60;
        }
        assert_eq!(
            limits.may_sign_in("alice", ip(2), now).await.unwrap(),
            Ok(())
        );
        for _ in 0..NAME_TRIES - 1 {
            limits.failed("alice", ip(2), now).await.unwrap();
            now += secs(NAME_WINDOW) / 4;
        }
        assert_eq!(
            limits.may_sign_in("alice", ip(2), now).await.unwrap(),
            Ok(())
        );
    }

    #[actix_rt::test]
    async fn one_address_gets_twenty_wrong_guesses_an_hour() {
        let limits = limits().await;
        for i in 0..IP_TRIES {
            limits
                .failed(&format!("user{i}"), ip(1), START)
                .await
                .unwrap();
        }
        assert_eq!(
            limits.may_sign_in("someone", ip(1), START).await.unwrap(),
            Err(Refused::TooManyFromAddress)
        );
        assert_eq!(
            limits.may_sign_in("someone", ip(2), START).await.unwrap(),
            Ok(())
        );
        let later = START + secs(IP_WINDOW);
        assert_eq!(
            limits.may_sign_in("someone", ip(1), later).await.unwrap(),
            Ok(())
        );
    }

    #[actix_rt::test]
    async fn one_address_makes_five_accounts_a_day() {
        let limits = limits().await;
        for _ in 0..SIGNUPS {
            assert_eq!(limits.may_sign_up(ip(1), START).await.unwrap(), Ok(()));
            limits.signed_up(ip(1), START).await.unwrap();
        }
        assert_eq!(
            limits.may_sign_up(ip(1), START).await.unwrap(),
            Err(Refused::TooManySignUps)
        );
        let tomorrow = START + secs(SIGNUP_WINDOW);
        limits.tidy(tomorrow).await.unwrap();
        assert_eq!(limits.may_sign_up(ip(1), tomorrow).await.unwrap(), Ok(()));
    }
}
