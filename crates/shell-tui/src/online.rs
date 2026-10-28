//! What the app knows about the player and the people and rooms around
//! them: their account, friends, the leaderboard, forms, and the room
//! they're in.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use game_api::{AnyView, GameId, PlayerId, SeatView, ShellView};
use zeroize::{Zeroize, Zeroizing};

/// Longest password, in characters; they're all ASCII.
pub const MAX_SECRET: usize = 128;

/// Rated games an account needs before the leaderboard lists it: new
/// ratings swing a lot.
pub const RANKED_AFTER: u32 = 5;

/// How long quick play waits for people before offering a match against
/// bots.
pub const BOTS_OFFERED_AFTER: Duration = Duration::from_secs(20);

/// A password while it's typed and sent: wiped from memory when dropped,
/// and never shown by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Default for Secret {
    fn default() -> Self {
        // Room for the longest password up front, so typing never moves it
        // and leaves a copy behind.
        Self(Zeroizing::new(String::with_capacity(MAX_SECRET)))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> Zeroizing<String> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn push(&mut self, c: char) {
        if self.0.len() < MAX_SECRET {
            self.0.push(c);
        }
    }

    pub(crate) fn pop(&mut self) {
        self.0.pop();
    }

    pub(crate) fn clear(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Code,
    Nickname,
    FriendName,
}

impl Field {
    pub fn max_len(self) -> usize {
        match self {
            Field::Code => 6,
            Field::Nickname | Field::FriendName => 16,
        }
    }

    /// Only characters that are safe to show on other players' screens.
    pub(crate) fn accepts(self, c: char) -> bool {
        match self {
            Field::Code => c.is_ascii_alphanumeric(),
            Field::Nickname | Field::FriendName => {
                c.is_ascii_alphanumeric() || c == '_' || c == '-'
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HomeItem {
    /// The open game's own entry, by its place in
    /// [`crate::GameUi::home_items`], such as Snake's single player.
    Game(usize),
    QuickPlay,
    CreateRoom,
    JoinCode,
    Friends,
    Account,
    Nickname,
    SignUp,
    SignIn,
    /// The game picker, when there's more than one game.
    Games,
    Leaderboard,
    Quit,
}

impl HomeItem {
    /// What the entry says; the game names its own.
    pub fn label(self) -> &'static str {
        match self {
            HomeItem::Game(_) => "",
            HomeItem::QuickPlay => "Quick play",
            HomeItem::CreateRoom => "Create a private room",
            HomeItem::JoinCode => "Join with a code",
            HomeItem::Friends => "Friends",
            HomeItem::Account => "Account",
            HomeItem::Nickname => "Nickname",
            HomeItem::SignUp => "Sign up",
            HomeItem::SignIn => "Sign in",
            HomeItem::Games => "Other games",
            HomeItem::Leaderboard => "Leaderboard",
            HomeItem::Quit => "Quit",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormKind {
    SignUp,
    SignIn,
    ChangePassword,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormRow {
    Username,
    Password,
    Repeat,
    Remember,
}

impl FormKind {
    pub fn title(self) -> &'static str {
        match self {
            FormKind::SignUp => " Sign up ",
            FormKind::SignIn => " Sign in ",
            FormKind::ChangePassword => " Change password ",
        }
    }

    /// The form's rows, top to bottom. Remembering the machine is only
    /// offered to players who connected with a key.
    pub fn rows(self, has_key: bool) -> Vec<FormRow> {
        match self {
            FormKind::SignUp => vec![FormRow::Username, FormRow::Password, FormRow::Repeat],
            FormKind::SignIn if has_key => {
                vec![FormRow::Username, FormRow::Password, FormRow::Remember]
            }
            FormKind::SignIn => vec![FormRow::Username, FormRow::Password],
            FormKind::ChangePassword => vec![FormRow::Password, FormRow::Repeat],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Form {
    pub username: String,
    pub password: Secret,
    pub repeat: Secret,
    pub remember: bool,
    /// Index into [`FormKind::rows`].
    pub focus: usize,
    pub error: Option<String>,
    /// Sent; waiting for the answer.
    pub busy: bool,
}

/// The signed-in account, as shown, with its rating in the open game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub username: String,
    pub rating: u32,
    pub games: u32,
    pub wins: u32,
}

/// A key the account is remembered by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRow {
    pub fingerprint: String,
    /// Already worded, such as "used today".
    pub last_used: String,
    pub this_machine: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FriendRow {
    pub name: String,
    /// What they're doing, or when they were last seen.
    pub status: String,
    pub online: bool,
    /// Playing in a room this player can watch.
    pub watch: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendsView {
    pub friends: Vec<FriendRow>,
    /// Asked to be friends; waiting for this player's answer.
    pub incoming: Vec<String>,
    /// This player asked; waiting for theirs.
    pub outgoing: Vec<String>,
}

/// One selectable row of the friends screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FriendsRow {
    Add,
    Request(usize),
    Friend(usize),
    Asked(usize),
}

impl FriendsView {
    pub fn rows(&self) -> Vec<FriendsRow> {
        let mut rows = vec![FriendsRow::Add];
        rows.extend((0..self.incoming.len()).map(FriendsRow::Request));
        rows.extend((0..self.friends.len()).map(FriendsRow::Friend));
        rows.extend((0..self.outgoing.len()).map(FriendsRow::Asked));
        rows
    }
}

/// One player on the leaderboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaderRow {
    /// 1 for the best.
    pub place: u32,
    pub name: String,
    pub rating: u32,
    pub games: u32,
    pub wins: u32,
}

/// Where this player stands on the leaderboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MyPlace {
    Guest,
    Unranked { games_left: u32 },
    Ranked(LeaderRow),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaderboardView {
    /// The best players, best first.
    pub rows: Vec<LeaderRow>,
    pub me: MyPlace,
}

/// One selectable row of the account screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccountRow {
    Key(usize),
    ChangePassword,
    SignOut,
    Back,
}

/// Where quick play looks for opponents first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueRegion {
    pub name: String,
    /// After waiting this long, players from every region are considered.
    pub widens_after: Duration,
}

/// Someone's invite to their room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    pub from: String,
    /// The room's game.
    pub game: GameId,
}

/// What the app knows about the player and the people and rooms around
/// them.
#[derive(Clone, Debug)]
pub struct Online {
    /// How others see this player: the username, or `~nick` for a guest.
    pub name: String,
    /// Shown on the current screen until the next key.
    pub notice: Option<String>,
    pub room: Option<InRoom>,
    /// Time spent in the queue so far.
    pub waited: Duration,
    pub queue_len: usize,
    /// In a cluster of servers: the region this player is matched in first.
    pub region: Option<QueueRegion>,
    pub account: Option<Profile>,
    /// Connected with an SSH key, which an account can remember.
    pub has_key: bool,
    pub keys: Vec<KeyRow>,
    pub friends: FriendsView,
    /// `None` until it loads.
    pub leaderboard: Option<LeaderboardView>,
    /// Who's inviting this player to their room, waiting for Enter or Esc.
    pub invite: Option<Invite>,
    /// Friend requests waiting for this player's answer.
    pub requests: usize,
    /// Quick play was chosen once on a terminal too small for its matches;
    /// choosing it again queues anyway.
    pub(crate) confirm_small: bool,
    /// x was pressed once on this friend; a second x removes them.
    pub confirm_remove: Option<String>,
    /// The friends list was opened from a private room's lobby.
    pub(crate) friends_from_room: bool,
}

impl Online {
    pub(crate) fn new(name: String, has_key: bool) -> Self {
        Self {
            name,
            notice: None,
            room: None,
            waited: Duration::ZERO,
            queue_len: 0,
            region: None,
            account: None,
            has_key,
            keys: Vec::new(),
            friends: FriendsView::default(),
            leaderboard: None,
            invite: None,
            requests: 0,
            confirm_small: false,
            confirm_remove: None,
            friends_from_room: false,
        }
    }

    pub fn account_rows(&self) -> Vec<AccountRow> {
        let mut rows: Vec<AccountRow> = (0..self.keys.len()).map(AccountRow::Key).collect();
        rows.extend([
            AccountRow::ChangePassword,
            AccountRow::SignOut,
            AccountRow::Back,
        ]);
        rows
    }
}

#[derive(Clone, Debug)]
pub struct InRoom {
    pub me: PlayerId,
    /// The room's latest view, of its game.
    pub view: Arc<dyn AnyView>,
    /// Esc during a round asks first.
    pub confirm_leave: bool,
    /// Only watching, without a seat: the friend this player follows.
    pub watching: Option<String>,
}

impl InRoom {
    /// The room around the round: seats, phase and results.
    pub fn shell(&self) -> &ShellView {
        self.view.shell()
    }

    pub fn my_seat(&self) -> Option<&SeatView> {
        self.shell().seat(self.me)
    }

    /// The seat this player's screen follows: their own, or the friend's
    /// they watch.
    pub fn followed(&self) -> Option<&SeatView> {
        match &self.watching {
            Some(friend) => self.shell().seats.iter().find(|s| s.who.name == *friend),
            None => self.my_seat(),
        }
    }

    /// Whether this player sets the room's rules.
    pub fn hosting(&self) -> bool {
        self.shell().host().is_some_and(|h| h.who.id == self.me)
    }
}
