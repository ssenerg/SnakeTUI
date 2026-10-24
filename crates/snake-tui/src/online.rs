//! The parts of the app only played over the network: the home menu, rooms,
//! accounts and friends.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use snake_core::config::GameConfig;
use snake_core::records::Records;
use snake_core::round::{PhaseView, PlayerId, RoomKind, RoomView, SeatView, quick_rules};
use zeroize::{Zeroize, Zeroizing};

use crate::app::{App, Effect, Input, Screen, direction};
use crate::key::Key;
use crate::ui;

/// Longest password, in characters; they're all ASCII.
pub const MAX_SECRET: usize = 128;

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

    fn push(&mut self, c: char) {
        if self.0.len() < MAX_SECRET {
            self.0.push(c);
        }
    }

    fn pop(&mut self) {
        self.0.pop();
    }

    fn clear(&mut self) {
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
    fn accepts(self, c: char) -> bool {
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
    SinglePlayer,
    QuickPlay,
    CreateRoom,
    JoinCode,
    Friends,
    Account,
    Nickname,
    SignUp,
    SignIn,
    Quit,
}

impl HomeItem {
    pub fn label(self) -> &'static str {
        match self {
            HomeItem::SinglePlayer => "Single player",
            HomeItem::QuickPlay => "Quick play",
            HomeItem::CreateRoom => "Create a private room",
            HomeItem::JoinCode => "Join with a code",
            HomeItem::Friends => "Friends",
            HomeItem::Account => "Account",
            HomeItem::Nickname => "Nickname",
            HomeItem::SignUp => "Sign up",
            HomeItem::SignIn => "Sign in",
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

/// The signed-in account, as shown.
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

/// One selectable row of the account screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccountRow {
    Key(usize),
    ChangePassword,
    SignOut,
    Back,
}

/// What an online player's app knows beyond the local game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Online {
    /// How others see this player: the username, or `~nick` for a guest.
    pub name: String,
    /// Shown on the current screen until the next key.
    pub notice: Option<String>,
    pub room: Option<InRoom>,
    /// Time spent in the queue so far.
    pub waited: Duration,
    pub queue_len: usize,
    pub account: Option<Profile>,
    /// Connected with an SSH key, which an account can remember.
    pub has_key: bool,
    pub keys: Vec<KeyRow>,
    pub friends: FriendsView,
    /// Who's inviting this player to their room, waiting for Enter or Esc.
    pub invite: Option<String>,
    /// Friend requests waiting for this player's answer.
    pub requests: usize,
    /// Quick play was chosen once on a terminal too small for the whole
    /// board; choosing it again queues anyway.
    confirm_small: bool,
    /// x was pressed once on this friend; a second x removes them.
    pub confirm_remove: Option<String>,
    /// The friends list was opened from a private room's lobby.
    friends_from_room: bool,
}

impl Online {
    pub(crate) fn new(name: String, has_key: bool) -> Self {
        Self {
            name,
            notice: None,
            room: None,
            waited: Duration::ZERO,
            queue_len: 0,
            account: None,
            has_key,
            keys: Vec::new(),
            friends: FriendsView::default(),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InRoom {
    pub me: PlayerId,
    pub view: Arc<RoomView>,
    /// Esc during a round asks first.
    pub confirm_leave: bool,
}

impl InRoom {
    pub fn my_seat(&self) -> Option<&SeatView> {
        self.view.seat(self.me)
    }

    /// Whether this player has a snake in the round being played.
    pub fn playing(&self) -> bool {
        matches!(
            self.view.phase,
            PhaseView::Countdown { .. } | PhaseView::Running { .. }
        ) && self.my_seat().is_some_and(|s| s.slot.is_some() && s.alive)
    }
}

/// Moves `selected` up or down a list of `len` rows, wrapping around.
fn step(selected: usize, len: usize, key: Key) -> Option<usize> {
    match key {
        Key::Up | Key::Char('w' | 'k') => Some(selected.checked_sub(1).unwrap_or(len - 1)),
        Key::Down | Key::Char('s' | 'j') => Some((selected + 1) % len),
        _ => None,
    }
}

impl App {
    /// The home menu's entries: guests can sign up or in, accounts see
    /// their friends.
    pub fn home_items(&self) -> Vec<HomeItem> {
        let mut items = vec![
            HomeItem::SinglePlayer,
            HomeItem::QuickPlay,
            HomeItem::CreateRoom,
            HomeItem::JoinCode,
        ];
        if self.online.as_ref().is_some_and(|o| o.account.is_some()) {
            items.extend([HomeItem::Friends, HomeItem::Account]);
        } else {
            items.extend([HomeItem::Nickname, HomeItem::SignUp, HomeItem::SignIn]);
        }
        items.push(HomeItem::Quit);
        items
    }

    pub(crate) fn go_home(&mut self, at: HomeItem, notice: Option<String>) {
        let selected = self.home_items().iter().position(|i| *i == at).unwrap_or(0);
        self.screen = Screen::Home { selected };
        if let Some(online) = &mut self.online {
            online.notice = notice;
            online.confirm_small = false;
        }
    }

    /// Whether the player is in the middle of playing, so an invite waits.
    fn busy_playing(&self) -> bool {
        self.screen == Screen::Playing
            || (self.screen == Screen::Room
                && self
                    .online
                    .as_ref()
                    .and_then(|o| o.room.as_ref())
                    .is_some_and(InRoom::playing))
    }

    /// Whether an invite is showing, so Enter and Esc answer it.
    pub fn invite_showing(&self) -> bool {
        self.online.as_ref().is_some_and(|o| o.invite.is_some()) && !self.busy_playing()
    }

    /// Enter accepts a showing invite and Esc dismisses it. Returns whether
    /// the key was used.
    pub(crate) fn invite_key(&mut self, key: Key, effects: &mut Vec<Effect>) -> bool {
        if !self.invite_showing() {
            return false;
        }
        match key {
            Key::Enter => {
                if self.screen == Screen::Finding {
                    effects.push(Effect::LeaveQueue);
                }
                if self.in_room() {
                    effects.push(Effect::LeaveRoom);
                    self.leave_room();
                }
                effects.push(Effect::AcceptInvite);
                if let Some(online) = &mut self.online {
                    online.invite = None;
                }
                self.screen = Screen::Joining { by_code: false };
                true
            }
            Key::Esc => {
                effects.push(Effect::DismissInvite);
                if let Some(online) = &mut self.online {
                    online.invite = None;
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn home_key(&mut self, selected: usize, key: Key, effects: &mut Vec<Effect>) {
        let items = self.home_items();
        let Some(online) = &mut self.online else {
            return;
        };
        let had_notice = online.notice.take().is_some();
        if let Some(selected) = step(selected, items.len(), key) {
            online.confirm_small = false;
            self.screen = Screen::Home { selected };
            return;
        }
        match key {
            Key::Enter | Key::Char(' ') => {
                let item = items[selected.min(items.len() - 1)];
                self.choose(item, effects);
            }
            Key::Esc | Key::Char('q') if !had_notice => effects.push(Effect::Quit),
            _ => {}
        }
    }

    fn choose(&mut self, item: HomeItem, effects: &mut Vec<Effect>) {
        let size = self.size;
        let Some(online) = &mut self.online else {
            return;
        };
        match item {
            HomeItem::SinglePlayer => self.screen = Screen::Menu,
            HomeItem::QuickPlay => {
                let rules = quick_rules();
                if !ui::room_fits(rules.width, rules.height, size) && !online.confirm_small {
                    let (w, h) = ui::room_size(rules.width, rules.height);
                    online.notice = Some(format!(
                        "Your terminal is {}×{}; matches need {w}×{h} to show the whole board. \
                         Press Enter again to play anyway.",
                        size.width, size.height
                    ));
                    online.confirm_small = true;
                    return;
                }
                online.confirm_small = false;
                online.waited = Duration::ZERO;
                self.screen = Screen::Finding;
                effects.push(Effect::QuickPlay);
            }
            HomeItem::CreateRoom => {
                self.screen = Screen::Joining { by_code: false };
                effects.push(Effect::CreateRoom(self.cfg.rules.clone()));
            }
            HomeItem::JoinCode => self.start_input(Field::Code, String::new()),
            HomeItem::Nickname => {
                let nick = online.name.trim_start_matches('~').to_string();
                self.start_input(Field::Nickname, nick);
            }
            HomeItem::Friends => {
                online.friends_from_room = false;
                online.confirm_remove = None;
                self.screen = Screen::Friends { selected: 0 };
                effects.push(Effect::LoadFriends);
            }
            HomeItem::Account => {
                self.screen = Screen::Account { selected: 0 };
                effects.push(Effect::LoadAccount);
            }
            HomeItem::SignUp => self.start_form(FormKind::SignUp),
            HomeItem::SignIn => self.start_form(FormKind::SignIn),
            HomeItem::Quit => effects.push(Effect::Quit),
        }
    }

    fn start_input(&mut self, field: Field, text: String) {
        self.input = Input { text, error: None };
        self.screen = Screen::Input(field);
    }

    fn start_form(&mut self, kind: FormKind) {
        self.form = Form {
            remember: true,
            ..Form::default()
        };
        self.screen = Screen::Form(kind);
    }

    pub(crate) fn input_key(&mut self, field: Field, key: Key, effects: &mut Vec<Effect>) {
        let input = &mut self.input;
        match key {
            Key::Char(c) if field.accepts(c) && input.text.len() < field.max_len() => {
                input.error = None;
                input.text.push(match field {
                    Field::Code => c.to_ascii_uppercase(),
                    _ => c,
                });
            }
            Key::Backspace => {
                input.error = None;
                input.text.pop();
            }
            Key::Esc => match field {
                Field::Code => self.go_home(HomeItem::JoinCode, None),
                Field::Nickname => self.go_home(HomeItem::Nickname, None),
                Field::FriendName => self.screen = Screen::Friends { selected: 0 },
            },
            Key::Enter => match field {
                Field::Code if input.text.len() != field.max_len() => {
                    input.error = Some(format!("Codes are {} characters.", field.max_len()));
                }
                Field::Code => {
                    effects.push(Effect::JoinRoom(input.text.clone()));
                    self.screen = Screen::Joining { by_code: true };
                }
                _ if input.text.is_empty() => input.error = Some("Type a name.".to_string()),
                Field::Nickname => {
                    effects.push(Effect::SetNickname(input.text.clone()));
                    self.go_home(HomeItem::Nickname, None);
                }
                Field::FriendName => {
                    effects.push(Effect::AddFriend(input.text.clone()));
                    self.screen = Screen::Friends { selected: 0 };
                }
            },
            _ => {}
        }
    }

    pub(crate) fn finding_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        if matches!(key, Key::Esc | Key::Char('q')) {
            effects.push(Effect::LeaveQueue);
            self.go_home(HomeItem::QuickPlay, None);
        }
    }

    pub(crate) fn room_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        let signed_in = self.online.as_ref().is_some_and(|o| o.account.is_some());
        let Some(online) = self.online.as_mut() else {
            return;
        };
        online.notice = None;
        let Some(room) = online.room.as_mut() else {
            return;
        };
        let leave = match &room.view.phase {
            PhaseView::Waiting { .. } => match key {
                Key::Enter | Key::Char(' ' | 'r') => {
                    effects.push(Effect::ToggleReady);
                    false
                }
                Key::Char('i') if signed_in => {
                    online.friends_from_room = true;
                    online.confirm_remove = None;
                    self.screen = Screen::Friends { selected: 0 };
                    effects.push(Effect::LoadFriends);
                    false
                }
                Key::Char('i') => {
                    online.notice = Some("Sign in to invite friends.".to_string());
                    false
                }
                Key::Esc | Key::Char('q') => true,
                _ => false,
            },
            PhaseView::Countdown { .. } | PhaseView::Running { .. } => {
                let playing = room.playing();
                match (direction(key), key) {
                    (Some(dir), _) if playing => {
                        effects.push(Effect::Steer(dir));
                        false
                    }
                    (_, Key::Char('y')) if room.confirm_leave => true,
                    (_, Key::Char('n') | Key::Esc) if room.confirm_leave => {
                        room.confirm_leave = false;
                        false
                    }
                    (_, Key::Esc | Key::Char('q')) if playing => {
                        room.confirm_leave = true;
                        false
                    }
                    (_, Key::Esc | Key::Char('q')) => true,
                    _ => false,
                }
            }
            PhaseView::Results { .. } => match key {
                Key::Enter | Key::Char(' ') if room.view.kind == RoomKind::Match => {
                    effects.push(Effect::LeaveRoom);
                    self.leave_room();
                    self.choose(HomeItem::QuickPlay, effects);
                    return;
                }
                Key::Esc | Key::Char('q') => true,
                _ => false,
            },
            PhaseView::Closed { .. } => true,
        };
        if leave {
            effects.push(Effect::LeaveRoom);
            self.leave_room();
        }
    }

    fn leave_room(&mut self) {
        let kind = self
            .online
            .as_mut()
            .and_then(|o| o.room.take())
            .map(|r| r.view.kind);
        let at = match kind {
            Some(RoomKind::Private) => HomeItem::CreateRoom,
            _ => HomeItem::QuickPlay,
        };
        self.go_home(at, None);
    }

    pub(crate) fn form_key(&mut self, kind: FormKind, key: Key, effects: &mut Vec<Effect>) {
        let has_key = self.online.as_ref().is_some_and(|o| o.has_key);
        let rows = kind.rows(has_key);
        let form = &mut self.form;
        if form.busy {
            return;
        }
        let row = rows[form.focus.min(rows.len() - 1)];
        match key {
            Key::Up => form.focus = form.focus.checked_sub(1).unwrap_or(rows.len() - 1),
            Key::Down => form.focus = (form.focus + 1) % rows.len(),
            Key::Esc => {
                *form = Form::default();
                match kind {
                    FormKind::ChangePassword => self.screen = Screen::Account { selected: 0 },
                    FormKind::SignUp => self.go_home(HomeItem::SignUp, None),
                    FormKind::SignIn => self.go_home(HomeItem::SignIn, None),
                }
            }
            Key::Char(' ') if row == FormRow::Remember => form.remember = !form.remember,
            Key::Char(c) => {
                form.error = None;
                match row {
                    FormRow::Username
                        if Field::Nickname.accepts(c)
                            && form.username.len() < Field::Nickname.max_len() =>
                    {
                        form.username.push(c);
                    }
                    FormRow::Password if c.is_ascii_graphic() || c == ' ' => form.password.push(c),
                    FormRow::Repeat if c.is_ascii_graphic() || c == ' ' => form.repeat.push(c),
                    _ => {}
                }
            }
            Key::Backspace => match row {
                FormRow::Username => {
                    form.username.pop();
                }
                FormRow::Password => form.password.pop(),
                FormRow::Repeat => form.repeat.pop(),
                FormRow::Remember => {}
            },
            Key::Enter if row != FormRow::Remember && form.focus + 1 < rows.len() => {
                form.focus += 1;
            }
            Key::Enter => self.submit_form(kind, effects),
            _ => {}
        }
    }

    fn submit_form(&mut self, kind: FormKind, effects: &mut Vec<Effect>) {
        let form = &mut self.form;
        let needs_name = kind != FormKind::ChangePassword;
        let problem = if needs_name && form.username.is_empty() {
            Some("Type a username.")
        } else if form.password.is_empty() {
            Some("Type a password.")
        } else if kind != FormKind::SignIn && form.password != form.repeat {
            Some("The passwords don't match.")
        } else {
            None
        };
        if let Some(problem) = problem {
            form.error = Some(problem.to_string());
            form.password.clear();
            form.repeat.clear();
            form.focus = usize::from(needs_name);
            return;
        }
        form.busy = true;
        form.error = None;
        let password = std::mem::take(&mut form.password);
        form.repeat.clear();
        effects.push(match kind {
            FormKind::SignUp => Effect::SignUp {
                username: form.username.clone(),
                password,
            },
            FormKind::SignIn => Effect::SignIn {
                username: form.username.clone(),
                password,
                remember: form.remember,
            },
            FormKind::ChangePassword => Effect::ChangePassword(password),
        });
    }

    pub(crate) fn account_key(&mut self, selected: usize, key: Key, effects: &mut Vec<Effect>) {
        let Some(online) = &mut self.online else {
            return;
        };
        online.notice = None;
        let rows = online.account_rows();
        if let Some(selected) = step(selected, rows.len(), key) {
            self.screen = Screen::Account { selected };
            return;
        }
        let row = rows[selected.min(rows.len() - 1)];
        match (row, key) {
            (AccountRow::Key(i), Key::Char('x' | 'd') | Key::Backspace) => {
                effects.push(Effect::ForgetKey(online.keys[i].fingerprint.clone()));
            }
            (AccountRow::ChangePassword, Key::Enter) => self.start_form(FormKind::ChangePassword),
            (AccountRow::SignOut, Key::Enter) => effects.push(Effect::SignOut),
            (AccountRow::Back, Key::Enter) | (_, Key::Esc | Key::Char('q')) => {
                self.go_home(HomeItem::Account, None);
            }
            _ => {}
        }
    }

    pub(crate) fn friends_key(&mut self, selected: usize, key: Key, effects: &mut Vec<Effect>) {
        let Some(online) = &mut self.online else {
            return;
        };
        online.notice = None;
        let rows = online.friends.rows();
        if let Some(selected) = step(selected, rows.len(), key) {
            online.confirm_remove = None;
            self.screen = Screen::Friends { selected };
            return;
        }
        let row = rows[selected.min(rows.len() - 1)];
        let confirm_remove = online.confirm_remove.take();
        match (row, key) {
            (FriendsRow::Add, Key::Enter) => self.start_input(Field::FriendName, String::new()),
            (FriendsRow::Request(i), Key::Enter | Key::Char('a')) => {
                effects.push(Effect::AnswerFriend {
                    name: online.friends.incoming[i].clone(),
                    accept: true,
                });
            }
            (FriendsRow::Request(i), Key::Char('d' | 'x')) => {
                effects.push(Effect::AnswerFriend {
                    name: online.friends.incoming[i].clone(),
                    accept: false,
                });
            }
            (FriendsRow::Friend(i), Key::Enter) => {
                let friend = &online.friends.friends[i];
                if !friend.online {
                    online.notice = Some(format!("{} is offline.", friend.name));
                } else {
                    effects.push(Effect::Invite(friend.name.clone()));
                    if online.friends_from_room && online.room.is_some() {
                        self.screen = Screen::Room;
                    } else {
                        if online.room.is_some() {
                            effects.push(Effect::LeaveRoom);
                            online.room = None;
                        }
                        self.screen = Screen::Joining { by_code: false };
                    }
                }
            }
            (FriendsRow::Friend(i), Key::Char('x')) => {
                let name = online.friends.friends[i].name.clone();
                if confirm_remove.as_deref() == Some(&name) {
                    effects.push(Effect::RemoveFriend(name));
                } else {
                    online.notice = Some(format!("Press x again to remove {name}."));
                    online.confirm_remove = Some(name);
                }
            }
            (FriendsRow::Asked(i), Key::Char('x')) => {
                effects.push(Effect::RemoveFriend(online.friends.outgoing[i].clone()));
            }
            (_, Key::Esc | Key::Char('q')) => {
                if online.friends_from_room && online.room.is_some() {
                    self.screen = Screen::Room;
                } else {
                    self.go_home(HomeItem::Friends, None);
                }
            }
            _ => {}
        }
    }

    // What the session tells the app.

    /// The name others see, as the server settled it.
    pub fn set_name(&mut self, name: String) {
        if let Some(online) = &mut self.online {
            online.name = name;
        }
    }

    /// Something to tell the player on the screen they're on.
    pub fn notify(&mut self, notice: String) {
        if let Some(online) = &mut self.online {
            online.notice = Some(notice);
        }
    }

    pub fn set_queue_len(&mut self, n: usize) {
        if let Some(online) = &mut self.online {
            online.queue_len = n;
        }
    }

    /// Whether the player is waiting in the queue, so a match is welcome.
    pub fn wants_match(&self) -> bool {
        self.screen == Screen::Finding
    }

    pub fn in_room(&self) -> bool {
        self.online.as_ref().is_some_and(|o| o.room.is_some())
    }

    /// The player is now seated in a room.
    pub fn enter_room(&mut self, me: PlayerId, view: Arc<RoomView>) {
        let Some(online) = &mut self.online else {
            return;
        };
        online.notice = None;
        online.room = Some(InRoom {
            me,
            view,
            confirm_leave: false,
        });
        self.screen = Screen::Room;
    }

    /// The room changed.
    pub fn update_room(&mut self, view: Arc<RoomView>) {
        if let PhaseView::Closed { reason } = &view.phase {
            return self.room_closed(Some(reason.clone()));
        }
        let Some(online) = &mut self.online else {
            return;
        };
        let Some(room) = online.room.as_mut() else {
            return;
        };
        let lobby = matches!(view.phase, PhaseView::Waiting { .. });
        room.view = view;
        if !room.playing() {
            room.confirm_leave = false;
        }
        // A round starting pulls the player back from the friends list.
        if !lobby && matches!(self.screen, Screen::Friends { .. } | Screen::Input(_)) {
            self.screen = Screen::Room;
        }
    }

    /// The room is gone; `reason` says why, if the room said.
    pub fn room_closed(&mut self, reason: Option<String>) {
        if self.in_room() {
            self.leave_room();
            if let Some(online) = &mut self.online {
                online.notice = reason;
            }
        }
    }

    /// Making or joining a private room didn't work.
    pub fn join_failed(&mut self, why: String) {
        match self.screen {
            Screen::Joining { by_code: true } => {
                self.input.error = Some(why);
                self.screen = Screen::Input(Field::Code);
            }
            Screen::Joining { by_code: false } => self.go_home(HomeItem::CreateRoom, Some(why)),
            _ => {}
        }
    }

    pub fn set_profile(&mut self, profile: Option<Profile>) {
        if let Some(online) = &mut self.online {
            if let Some(p) = &profile {
                online.name = p.username.clone();
            }
            online.account = profile;
        }
    }

    /// A form was refused; `why` says so, and passwords are cleared.
    pub fn form_failed(&mut self, why: String) {
        let form = &mut self.form;
        form.busy = false;
        form.error = Some(why);
        form.password.clear();
        form.repeat.clear();
        form.focus = 0;
    }

    /// The new account is ready; the guest's settings and records came
    /// along.
    pub fn signed_up(&mut self, profile: Profile) {
        let notice = format!("Welcome, {}! Your account is ready.", profile.username);
        self.form = Form::default();
        self.set_profile(Some(profile));
        self.go_home(HomeItem::QuickPlay, Some(notice));
    }

    /// Signed in: the account's settings and records replace the guest's.
    pub fn signed_in(&mut self, profile: Profile, cfg: GameConfig, records: Records) {
        let notice = format!("Signed in as {}.", profile.username);
        self.form = Form::default();
        self.cfg = cfg;
        self.records = records;
        self.set_profile(Some(profile));
        self.go_home(HomeItem::QuickPlay, Some(notice));
    }

    /// Back to being the guest `name`, with these settings and records.
    pub fn signed_out(&mut self, name: String, cfg: GameConfig, records: Records) {
        self.cfg = cfg;
        self.records = records;
        if let Some(online) = &mut self.online {
            online.account = None;
            online.name = name;
            online.keys.clear();
            online.friends = FriendsView::default();
            online.requests = 0;
            online.invite = None;
        }
        self.go_home(HomeItem::SignIn, Some("Signed out.".to_string()));
    }

    pub fn password_changed(&mut self) {
        self.form = Form::default();
        self.screen = Screen::Account { selected: 0 };
        self.notify("Password changed.".to_string());
    }

    pub fn set_keys(&mut self, keys: Vec<KeyRow>) {
        let Some(online) = &mut self.online else {
            return;
        };
        online.keys = keys;
        if let Screen::Account { selected } = self.screen {
            let last = online.account_rows().len() - 1;
            self.screen = Screen::Account {
                selected: selected.min(last),
            };
        }
    }

    pub fn set_friends(&mut self, friends: FriendsView) {
        let Some(online) = &mut self.online else {
            return;
        };
        online.requests = friends.incoming.len();
        online.friends = friends;
        if let Screen::Friends { selected } = self.screen {
            let last = online.friends.rows().len() - 1;
            self.screen = Screen::Friends {
                selected: selected.min(last),
            };
        }
    }

    /// `from` invites this player to their room.
    pub fn invited(&mut self, from: String) {
        if let Some(online) = &mut self.online {
            online.invite = Some(from);
        }
    }

    /// The invite expired or its room went away.
    pub fn invite_gone(&mut self) {
        if let Some(online) = &mut self.online {
            online.invite = None;
        }
    }

    pub fn friend_requested(&mut self, from: &str) {
        if let Some(online) = &mut self.online {
            online.requests += 1;
            online.notice = Some(format!(
                "{from} wants to be friends. Open Friends to answer."
            ));
        }
    }

    /// A rated round moved this player's rating.
    pub fn rated(&mut self, rating: u32, won: bool) {
        if let Some(profile) = self.online.as_mut().and_then(|o| o.account.as_mut()) {
            profile.rating = rating;
            profile.games += 1;
            profile.wins += u32::from(won);
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Size;
    use snake_core::geom::Direction;
    use snake_core::round::{Entrant, Round, RoundSettings};

    use super::*;

    fn online() -> App {
        let mut app = App::online(
            GameConfig::default(),
            Records::default(),
            "~alice".into(),
            true,
        );
        app.resize(Size::new(120, 40));
        app
    }

    fn signed_in() -> App {
        let mut app = online();
        app.set_profile(Some(Profile {
            username: "alice".into(),
            rating: 1000,
            games: 0,
            wins: 0,
        }));
        app
    }

    fn choose(app: &mut App, item: HomeItem) -> Vec<Effect> {
        let at = app.home_items().iter().position(|i| *i == item).unwrap();
        app.screen = Screen::Home { selected: at };
        app.handle(Key::Enter)
    }

    fn type_in(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle(Key::Char(c));
        }
    }

    #[test]
    fn online_single_player_goes_back_home_instead_of_quitting() {
        let mut app = online();
        app.handle(Key::Enter);
        assert_eq!(app.screen, Screen::Menu);
        assert_eq!(app.handle(Key::Char('q')), []);
        assert_eq!(app.screen, Screen::Home { selected: 0 });
        assert_eq!(app.handle(Key::Char('q')), [Effect::Quit]);
    }

    #[test]
    fn codes_are_typed_in_capitals_and_must_be_complete() {
        let mut app = online();
        choose(&mut app, HomeItem::JoinCode);
        assert_eq!(app.screen, Screen::Input(Field::Code));
        type_in(&mut app, "ab-c");
        assert_eq!(app.input.text, "ABC");
        app.handle(Key::Enter);
        assert!(app.input.error.is_some());
        type_in(&mut app, "d2k9");
        assert_eq!(app.input.text, "ABCD2K", "six at most");
        assert_eq!(app.handle(Key::Enter), [Effect::JoinRoom("ABCD2K".into())]);
        assert_eq!(app.screen, Screen::Joining { by_code: true });
        app.join_failed("No room has that code.".into());
        assert_eq!(app.screen, Screen::Input(Field::Code));
        assert_eq!(app.input.error.as_deref(), Some("No room has that code."));
    }

    #[test]
    fn quick_play_on_a_small_terminal_warns_first() {
        let mut app = online();
        app.resize(Size::new(60, 20));
        assert_eq!(choose(&mut app, HomeItem::QuickPlay), []);
        assert!(app.online.as_ref().unwrap().notice.is_some());
        assert_eq!(app.handle(Key::Enter), [Effect::QuickPlay]);
        assert_eq!(app.screen, Screen::Finding);
        app.tick(Duration::from_secs(2));
        assert_eq!(app.online.as_ref().unwrap().waited, Duration::from_secs(2));
        assert_eq!(app.handle(Key::Esc), [Effect::LeaveQueue]);
    }

    fn matched_view() -> Arc<RoomView> {
        let settings = RoundSettings::new(quick_rules(), RoomKind::Match).unwrap();
        let players = [1, 2]
            .map(|id| Entrant {
                id,
                name: format!("~p{id}"),
            })
            .to_vec();
        Arc::new(Round::new_match(settings, 1, players).view())
    }

    #[test]
    fn leaving_a_match_asks_first_while_the_snake_is_alive() {
        let mut app = online();
        app.screen = Screen::Finding;
        app.enter_room(1, matched_view());
        assert_eq!(app.screen, Screen::Room);
        assert_eq!(app.handle(Key::Up), [Effect::Steer(Direction::Up)]);
        assert_eq!(app.handle(Key::Esc), []);
        assert!(
            app.online
                .as_ref()
                .unwrap()
                .room
                .as_ref()
                .unwrap()
                .confirm_leave
        );
        assert_eq!(
            app.handle(Key::Left),
            [Effect::Steer(Direction::Left)],
            "still steering"
        );
        assert_eq!(app.handle(Key::Char('n')), []);
        app.handle(Key::Esc);
        assert_eq!(app.handle(Key::Char('y')), [Effect::LeaveRoom]);
        assert_eq!(app.screen, Screen::Home { selected: 1 });
    }

    #[test]
    fn a_closed_room_sends_the_player_home_with_the_reason() {
        let mut app = online();
        app.enter_room(1, matched_view());
        let mut closed = (*matched_view()).clone();
        closed.phase = PhaseView::Closed {
            reason: "The match is over.".into(),
        };
        app.update_room(Arc::new(closed));
        assert!(!app.in_room());
        assert_eq!(app.screen, Screen::Home { selected: 1 });
        assert_eq!(
            app.online.as_ref().unwrap().notice.as_deref(),
            Some("The match is over.")
        );
    }

    #[test]
    fn guests_can_sign_up_and_accounts_see_friends() {
        let guest = online();
        let items = guest.home_items();
        assert!(items.contains(&HomeItem::SignUp) && !items.contains(&HomeItem::Friends));
        let account = signed_in();
        let items = account.home_items();
        assert!(items.contains(&HomeItem::Friends) && !items.contains(&HomeItem::SignIn));
        assert_eq!(
            account.online.as_ref().unwrap().name,
            "alice",
            "no ~ for accounts"
        );
    }

    #[test]
    fn sign_up_checks_the_passwords_match_and_never_shows_them() {
        let mut app = online();
        choose(&mut app, HomeItem::SignUp);
        assert_eq!(app.screen, Screen::Form(FormKind::SignUp));
        type_in(&mut app, "Bob 1!");
        assert_eq!(app.form.username, "Bob1", "only name characters");
        app.handle(Key::Enter);
        type_in(&mut app, "correct horse");
        app.handle(Key::Enter);
        type_in(&mut app, "correct hors");
        assert_eq!(app.handle(Key::Enter), []);
        assert_eq!(
            app.form.error.as_deref(),
            Some("The passwords don't match.")
        );
        assert!(app.form.password.is_empty(), "cleared after a problem");
        type_in(&mut app, "correct horse");
        app.handle(Key::Enter);
        type_in(&mut app, "correct horse");
        let effects = app.handle(Key::Enter);
        let [Effect::SignUp { username, password }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(
            (username.as_str(), password.expose()),
            ("Bob1", "correct horse")
        );
        assert!(!format!("{effects:?}").contains("horse"), "Debug hides it");
        assert!(app.form.busy);
        assert_eq!(app.handle(Key::Char('x')), [], "waiting for the answer");
        app.form_failed("That username is taken.".into());
        assert!(!app.form.busy);
    }

    #[test]
    fn sign_in_offers_to_remember_only_a_machine_with_a_key() {
        assert_eq!(
            FormKind::SignIn.rows(false),
            [FormRow::Username, FormRow::Password]
        );
        let mut app = online();
        choose(&mut app, HomeItem::SignIn);
        type_in(&mut app, "alice");
        app.handle(Key::Enter);
        type_in(&mut app, "secret pw");
        app.handle(Key::Down);
        app.handle(Key::Char(' '));
        let effects = app.handle(Key::Enter);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SignIn {
                remember: false,
                ..
            }]
        ));
        app.signed_in(
            Profile {
                username: "alice".into(),
                rating: 1040,
                games: 3,
                wins: 1,
            },
            GameConfig::default(),
            Records::default(),
        );
        assert!(app.home_items().contains(&HomeItem::Account));
    }

    #[test]
    fn friends_are_invited_removed_on_a_second_x_and_requests_answered() {
        let mut app = signed_in();
        assert_eq!(choose(&mut app, HomeItem::Friends), [Effect::LoadFriends]);
        app.set_friends(FriendsView {
            friends: vec![
                FriendRow {
                    name: "bob".into(),
                    status: "online".into(),
                    online: true,
                },
                FriendRow {
                    name: "cat".into(),
                    status: "last seen today".into(),
                    online: false,
                },
            ],
            incoming: vec!["dan".into()],
            outgoing: vec!["eve".into()],
        });
        assert_eq!(app.online.as_ref().unwrap().requests, 1);
        // Add, dan's request, bob, cat, eve's.
        app.handle(Key::Down);
        assert_eq!(
            app.handle(Key::Char('a')),
            [Effect::AnswerFriend {
                name: "dan".into(),
                accept: true
            }]
        );
        app.handle(Key::Down);
        app.handle(Key::Down);
        assert_eq!(app.handle(Key::Enter), [], "cat is offline");
        app.handle(Key::Up);
        assert_eq!(app.handle(Key::Char('x')), []);
        assert_eq!(
            app.handle(Key::Char('x')),
            [Effect::RemoveFriend("bob".into())]
        );
        assert_eq!(app.handle(Key::Enter), [Effect::Invite("bob".into())]);
        assert_eq!(app.screen, Screen::Joining { by_code: false });
    }

    #[test]
    fn invites_wait_during_a_match_then_enter_accepts() {
        let mut app = signed_in();
        app.screen = Screen::Finding;
        app.enter_room(1, matched_view());
        app.invited("bob".into());
        assert!(!app.invite_showing(), "waits while the snake is in play");
        assert_eq!(app.handle(Key::Up), [Effect::Steer(Direction::Up)]);
        app.go_home(HomeItem::QuickPlay, None);
        app.online.as_mut().unwrap().room = None;
        assert!(app.invite_showing());
        assert_eq!(app.handle(Key::Enter), [Effect::AcceptInvite]);
        assert_eq!(app.screen, Screen::Joining { by_code: false });

        app.invited("cat".into());
        app.screen = Screen::Finding;
        assert_eq!(app.handle(Key::Esc), [Effect::DismissInvite]);
        assert_eq!(app.screen, Screen::Finding, "Esc went to the invite");
    }
}
