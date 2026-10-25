//! Signing up and signing in: what usernames and passwords may be, hashing
//! passwords, and how many wrong guesses anyone gets.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use snake_core::round::bot_names;
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use crate::accept::counted_as;
use crate::identity::GENERIC_USERS;
use crate::store::{AccountId, Counted, Store};

pub const MIN_USERNAME: usize = 3;
pub const MAX_USERNAME: usize = 16;
pub const MIN_PASSWORD: usize = 8;
pub const MAX_PASSWORD: usize = 128;

/// Names nobody may take, on top of the generic login names and the bots'.
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
    let bot = bot_names().iter().any(|b| b.eq_ignore_ascii_case(&lower));
    if GENERIC_USERS.contains(&lower.as_str()) || RESERVED.contains(&lower.as_str()) || bot {
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
/// Wrong room codes allowed from one IP address, however many connections
/// they come from.
pub const CODE_TRIES: usize = 20;
pub const CODE_WINDOW: Duration = Duration::from_secs(60 * 60);
/// Friend requests one account may send an hour, and how many a day to
/// the same account, however often they're taken back.
pub const FRIEND_ASKS: usize = 20;
pub const FRIEND_ASK_WINDOW: Duration = Duration::from_secs(60 * 60);
pub const SAME_ASKS: usize = 3;
pub const SAME_ASK_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
/// Invites one account may send in 10 minutes, and how many to the same
/// friend.
pub const INVITES: usize = 20;
pub const SAME_INVITES: usize = 3;
pub const INVITE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Tries to make an account one IP address may make, taken names and all.
pub const SIGNUP_TRIES: usize = 30;
pub const SIGNUP_TRY_WINDOW: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// This username had too many wrong guesses; it's locked for a while.
    NameLocked,
    /// This address made too many wrong guesses.
    TooManyFromAddress,
    /// This address made too many accounts today.
    TooManySignUps,
    /// This address tried to make too many accounts lately.
    TooManySignUpTries,
    /// Another try for this name, or from this address, is still going.
    Busy,
    /// This address tried too many wrong room codes.
    TooManyCodes,
    /// This account sent too many friend requests lately.
    TooManyAsks,
    /// This account asked the same one too often lately.
    AskedThemLately,
    /// This account sent too many invites lately.
    TooManyInvites,
    /// This account invited the same friend too often lately.
    InvitedThemLately,
}

impl Refused {
    pub fn message(self) -> &'static str {
        match self {
            Refused::NameLocked => "Too many wrong tries for that name. Try again in 15 minutes.",
            Refused::TooManyFromAddress => {
                "Too many wrong tries from your address. Try again later."
            }
            Refused::TooManySignUps => "Too many new accounts from your address today.",
            Refused::TooManySignUpTries => {
                "Too many tries to make an account from your address. Try again later."
            }
            Refused::Busy => "Another try is still going. Try again in a moment.",
            Refused::TooManyCodes => "Too many wrong codes from your address. Try again later.",
            Refused::TooManyAsks => "You've sent a lot of friend requests. Try again later.",
            Refused::AskedThemLately => "You've asked them a lot today. Give them a while.",
            Refused::TooManyInvites => "You've sent a lot of invites. Try again in a few minutes.",
            Refused::InvitedThemLately => "You've invited them a lot. Give them a few minutes.",
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
    /// Names and addresses with a try going on this server.
    busy: Arc<Mutex<HashSet<String>>>,
}

/// A sign-in or sign-up going on. While it lasts, no other try for its
/// name or from its address starts here, so each try is counted before
/// the next one is judged: tries sent all at once can't slip past the
/// limits together.
pub struct Attempt {
    held: [String; 2],
    busy: Arc<Mutex<HashSet<String>>>,
}

impl Drop for Attempt {
    fn drop(&mut self) {
        let mut busy = self.busy.lock().unwrap();
        for key in &self.held {
            busy.remove(key);
        }
    }
}

fn secs(d: Duration) -> i64 {
    d.as_secs() as i64
}

impl GuessLimits {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            busy: Arc::default(),
        }
    }

    /// Starts a try for `name` from `from`, unless one is going already.
    pub fn begin(&self, name: &str, from: IpAddr) -> Result<Attempt, Refused> {
        let held = [
            format!("name {}", name.to_ascii_lowercase()),
            format!("address {}", counted_as(from)),
        ];
        let mut busy = self.busy.lock().unwrap();
        if held.iter().any(|key| busy.contains(key)) {
            return Err(Refused::Busy);
        }
        busy.extend(held.iter().cloned());
        Ok(Attempt {
            held,
            busy: Arc::clone(&self.busy),
        })
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
                &counted_as(from).to_string(),
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
            .note(
                Counted::WrongFromAddress,
                &counted_as(from).to_string(),
                now,
            )
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

    /// Whether `from` may try to make an account. Every try counts.
    pub async fn may_sign_up(&self, from: IpAddr, now: i64) -> Result<Result<(), Refused>> {
        let address = counted_as(from).to_string();
        let tried = self
            .store
            .count(Counted::SignUpTry, &address, now - secs(SIGNUP_TRY_WINDOW))
            .await?;
        if tried >= SIGNUP_TRIES as i64 {
            return Ok(Err(Refused::TooManySignUpTries));
        }
        self.store.note(Counted::SignUpTry, &address, now).await?;
        let made = self
            .store
            .count(Counted::SignUp, &address, now - secs(SIGNUP_WINDOW))
            .await?;
        Ok(if made >= SIGNUPS as i64 {
            Err(Refused::TooManySignUps)
        } else {
            Ok(())
        })
    }

    pub async fn signed_up(&self, from: IpAddr, now: i64) -> Result<()> {
        self.store
            .note(Counted::SignUp, &counted_as(from).to_string(), now)
            .await
    }

    /// Whether `from` may try another room code.
    pub async fn may_try_code(&self, from: IpAddr, now: i64) -> Result<Result<(), Refused>> {
        let wrong = self
            .store
            .count(
                Counted::WrongCode,
                &counted_as(from).to_string(),
                now - secs(CODE_WINDOW),
            )
            .await?;
        Ok(if wrong >= CODE_TRIES as i64 {
            Err(Refused::TooManyCodes)
        } else {
            Ok(())
        })
    }

    pub async fn wrong_code(&self, from: IpAddr, now: i64) -> Result<()> {
        self.store
            .note(Counted::WrongCode, &counted_as(from).to_string(), now)
            .await
    }

    /// Whether `from` may ask `to` to be friends; counts it if so.
    pub async fn may_ask_friend(
        &self,
        from: AccountId,
        to: AccountId,
        now: i64,
    ) -> Result<Result<(), Refused>> {
        let limits = [
            (
                Counted::FriendAsk,
                from.to_string(),
                FRIEND_ASKS,
                FRIEND_ASK_WINDOW,
                Refused::TooManyAsks,
            ),
            (
                Counted::FriendAskPair,
                format!("{from}>{to}"),
                SAME_ASKS,
                SAME_ASK_WINDOW,
                Refused::AskedThemLately,
            ),
        ];
        self.within(&limits, now).await
    }

    /// Whether `from` may invite `to`; counts it if so.
    pub async fn may_invite(
        &self,
        from: AccountId,
        to: AccountId,
        now: i64,
    ) -> Result<Result<(), Refused>> {
        let limits = [
            (
                Counted::Invite,
                from.to_string(),
                INVITES,
                INVITE_WINDOW,
                Refused::TooManyInvites,
            ),
            (
                Counted::InvitePair,
                format!("{from}>{to}"),
                SAME_INVITES,
                INVITE_WINDOW,
                Refused::InvitedThemLately,
            ),
        ];
        self.within(&limits, now).await
    }

    /// Whether each count is under its limit, noting one more of each if
    /// they all are.
    async fn within(
        &self,
        limits: &[(Counted, String, usize, Duration, Refused)],
        now: i64,
    ) -> Result<Result<(), Refused>> {
        for (what, subject, most, window, refused) in limits {
            if self
                .store
                .count(*what, subject, now - secs(*window))
                .await?
                >= *most as i64
            {
                return Ok(Err(*refused));
            }
        }
        for (what, subject, ..) in limits {
            self.store.note(*what, subject, now).await?;
        }
        Ok(Ok(()))
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
        assert!(check_username("viper").is_err(), "a bot's name");
        assert!(check_username("PYTHON").is_err(), "a bot's name");
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
    async fn one_try_at_a_time_per_name_and_per_address() {
        let limits = limits().await;
        let first = limits.begin("Ann", ip(1)).unwrap();
        assert_eq!(
            limits.begin("ann", ip(2)).err(),
            Some(Refused::Busy),
            "the same name"
        );
        assert_eq!(
            limits.begin("ben", ip(1)).err(),
            Some(Refused::Busy),
            "the same address"
        );
        let other = limits.begin("ben", ip(2)).unwrap();
        drop(first);
        assert!(limits.begin("ANN", ip(1)).is_ok(), "free again");
        drop(other);
    }

    #[actix_rt::test]
    async fn one_ipv6_network_gets_twenty_wrong_room_codes_an_hour() {
        let limits = limits().await;
        let host = |n: u16| IpAddr::from([0x2001, 0xdb8, 7, 7, n, 0, 0, 1]);
        for n in 0..CODE_TRIES as u16 {
            assert_eq!(limits.may_try_code(host(n), 0).await.unwrap(), Ok(()));
            limits.wrong_code(host(n), 0).await.unwrap();
        }
        assert_eq!(
            limits.may_try_code(host(999), 0).await.unwrap(),
            Err(Refused::TooManyCodes),
            "any address in the same /64"
        );
        let other = IpAddr::from([0x2001, 0xdb8, 7, 8, 0, 0, 0, 1]);
        assert_eq!(limits.may_try_code(other, 0).await.unwrap(), Ok(()));
    }

    #[actix_rt::test]
    async fn friend_requests_and_invites_are_limited() {
        let limits = limits().await;
        // Asking one account again and again, taking it back between.
        for _ in 0..SAME_ASKS {
            assert_eq!(limits.may_ask_friend(1, 2, 0).await.unwrap(), Ok(()));
        }
        assert_eq!(
            limits.may_ask_friend(1, 2, 0).await.unwrap(),
            Err(Refused::AskedThemLately)
        );
        // Asking everyone.
        for to in 3..(3 + FRIEND_ASKS as i64 - SAME_ASKS as i64) {
            assert_eq!(limits.may_ask_friend(1, to, 0).await.unwrap(), Ok(()));
        }
        assert_eq!(
            limits.may_ask_friend(1, 999, 0).await.unwrap(),
            Err(Refused::TooManyAsks)
        );
        assert_eq!(
            limits.may_ask_friend(2, 1, 0).await.unwrap(),
            Ok(()),
            "others can"
        );
        let tomorrow = secs(SAME_ASK_WINDOW) + 1;
        assert_eq!(limits.may_ask_friend(1, 2, tomorrow).await.unwrap(), Ok(()));

        for _ in 0..SAME_INVITES {
            assert_eq!(limits.may_invite(1, 2, 0).await.unwrap(), Ok(()));
        }
        assert_eq!(
            limits.may_invite(1, 2, 0).await.unwrap(),
            Err(Refused::InvitedThemLately)
        );
        let later = secs(INVITE_WINDOW) + 1;
        assert_eq!(limits.may_invite(1, 2, later).await.unwrap(), Ok(()));
    }

    #[actix_rt::test]
    async fn every_try_to_make_an_account_counts() {
        let limits = limits().await;
        for _ in 0..SIGNUP_TRIES {
            assert_eq!(limits.may_sign_up(ip(1), 0).await.unwrap(), Ok(()));
        }
        assert_eq!(
            limits.may_sign_up(ip(1), 0).await.unwrap(),
            Err(Refused::TooManySignUpTries),
            "taken names count too"
        );
        let later = secs(SIGNUP_TRY_WINDOW) + 1;
        assert_eq!(limits.may_sign_up(ip(1), later).await.unwrap(), Ok(()));
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
