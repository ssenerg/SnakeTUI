//! The app's state machine: which screen shows, what keys do on it, and
//! what the server tells it. The open game's [`GameUi`] handles its own
//! screens and its part of rooms.

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use game_api::{AnyView, GameId, PhaseView, PlayerId, RatingKey, RoomKind, is_bot};
use ratatui::layout::Size;

use crate::game::{Ctx, GameFx, GameUi, Next, QuickStart, Reply, Saved};
use crate::key::Key;
use crate::online::{
    AccountRow, Field, Form, FormKind, FormRow, FriendsRow, FriendsView, HomeItem, InRoom, Invite,
    KeyRow, LeaderboardView, Online, Profile, QueueRegion, Secret, ShownRating,
};

/// How long quick play waits for people before offering a match against
/// bots, in games that have them.
pub const BOTS_OFFERED_AFTER: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    /// The games to choose from, at entry `selected`.
    Picker { selected: usize },
    /// The main menu, at the [`App::home_items`] entry `selected`.
    Home { selected: usize },
    /// Typing into [`App::input`].
    Input(Field),
    /// In the quick-play queue.
    Finding,
    /// Waiting to get into a private room.
    Joining { by_code: bool },
    /// In a room. What shows follows the room's phase.
    Room,
    /// Filling in [`App::form`].
    Form(FormKind),
    /// The signed-in account.
    Account { selected: usize },
    /// Friends and friend requests.
    Friends { selected: usize },
    /// The best players by the open game's rating number `category`, in
    /// [`GameUi::ratings`].
    Leaderboard { category: usize },
    /// On the way to watching a friend's game.
    JoiningToWatch,
    /// The open game's own screens, such as Snake's single player, or a
    /// room's rules.
    Game,
}

/// Work the app leaves to whoever runs it, in the order given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Work for this game.
    Game(GameId, GameFx),
    /// Leave the app.
    Quit,
    /// Join quick play in this game's `pool`; answer with
    /// [`App::enter_room`].
    QuickPlay {
        game: GameId,
        pool: &'static str,
    },
    LeaveQueue,
    /// Make a private room of `game` with these settings, as its rooms
    /// read them, and join it; answer with [`App::enter_room`] or
    /// [`App::join_failed`].
    CreateRoom {
        game: GameId,
        settings: Vec<u8>,
    },
    /// Join the private room with this code, of any game.
    JoinRoom(String),
    ToggleReady,
    /// The host adds a bot to the room.
    AddBot,
    /// The host takes the last bot out.
    RemoveBot,
    /// Leave the queue for an unrated match against bots; answer with
    /// [`App::enter_room`].
    PlayBots,
    /// Watch this friend's game; answer with [`App::watch_room`] or
    /// [`App::watch_failed`].
    Watch(String),
    LeaveRoom,
    /// Answer with [`App::set_name`].
    SetNickname(String),
    /// Answer with [`App::signed_up`] or [`App::form_failed`].
    SignUp {
        username: String,
        password: Secret,
    },
    /// Answer with [`App::signed_in`] or [`App::form_failed`].
    SignIn {
        username: String,
        password: Secret,
        /// Remember this machine's key for the account.
        remember: bool,
    },
    /// Answer with [`App::signed_out`].
    SignOut,
    /// Answer with [`App::password_changed`] or [`App::form_failed`].
    ChangePassword(Secret),
    /// Stop remembering the account by this key; answer with
    /// [`App::set_keys`].
    ForgetKey(String),
    /// Answer with [`App::set_keys`].
    LoadAccount,
    /// Answer with [`App::set_friends`].
    LoadFriends,
    /// The best players by this rating; answer with
    /// [`App::set_leaderboard`].
    LoadLeaderboard(RatingKey),
    AddFriend(String),
    AnswerFriend {
        name: String,
        accept: bool,
    },
    /// Ends a friendship, or takes back a request.
    RemoveFriend(String),
    /// Invite this friend to the private room the player is in, or to a
    /// new one: see [`App::room_to_make`].
    Invite(String),
    AcceptInvite,
    DismissInvite,
}

/// A room's phase, as its keys go.
#[derive(Clone, Copy)]
enum Phase {
    Lobby,
    Round,
    Results,
    Closed,
}

impl Phase {
    fn of(phase: &PhaseView) -> Self {
        match phase {
            PhaseView::Waiting { .. } => Phase::Lobby,
            PhaseView::Countdown { .. } | PhaseView::Running { .. } => Phase::Round,
            PhaseView::Results { .. } => Phase::Results,
            PhaseView::Closed { .. } => Phase::Closed,
        }
    }
}

/// What's being typed on an input screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    pub error: Option<String>,
}

pub struct App {
    pub screen: Screen,
    /// The terminal the app is drawn on. Set it with [`Self::resize`].
    pub size: Size,
    pub online: Online,
    pub input: Input,
    pub form: Form,
    games: Vec<Box<dyn GameUi>>,
    /// Index into `games` of the open game.
    open: usize,
    /// Whether a game has been opened yet. Until one is, the picker is
    /// all there is.
    opened: bool,
    /// The home entry the open game's own screens were opened from, which
    /// leaving them goes back to.
    game_item: HomeItem,
}

impl App {
    /// The app as the guest `name`, with these games. It opens `start` if
    /// it's one of them, or else the only game if there's one; otherwise
    /// the player picks.
    /// `has_key` says whether the player connected with an SSH key, which
    /// an account can then remember.
    pub fn online(
        games: Vec<Box<dyn GameUi>>,
        start: Option<GameId>,
        name: String,
        has_key: bool,
    ) -> Self {
        assert!(!games.is_empty(), "the app needs a game");
        let mut app = Self {
            screen: Screen::Picker { selected: 0 },
            size: Size::new(0, 0),
            online: Online::new(name, has_key),
            input: Input::default(),
            form: Form::default(),
            games,
            open: 0,
            opened: false,
            game_item: HomeItem::Game(0),
        };
        let start = start.and_then(|game| app.index_of(game));
        if let Some(i) = start.or_else(|| (app.games.len() == 1).then_some(0)) {
            app.switch_to(i);
            app.screen = Screen::Home { selected: 0 };
        }
        app
    }

    fn index_of(&self, game: GameId) -> Option<usize> {
        self.games.iter().position(|g| g.id() == game)
    }

    /// Opens `games[i]`.
    fn switch_to(&mut self, i: usize) {
        if i != self.open {
            self.open = i;
            self.game_item = HomeItem::Game(0);
        }
        self.opened = true;
    }

    /// The open game; `None` while the player has yet to pick one.
    pub fn game(&self) -> Option<GameId> {
        self.opened.then(|| self.games[self.open].id())
    }

    /// The open game's screens. Before one is picked, the first game's.
    pub fn ui(&self) -> &dyn GameUi {
        &*self.games[self.open]
    }

    /// Every game, in the picker's order.
    pub fn games(&self) -> impl Iterator<Item = &dyn GameUi> {
        self.games.iter().map(|g| &**g)
    }

    /// The game whose screens are a `T`.
    pub fn game_ui<T: GameUi>(&self) -> Option<&T> {
        self.games
            .iter()
            .find_map(|g| (&**g as &dyn Any).downcast_ref())
    }

    pub fn game_ui_mut<T: GameUi>(&mut self) -> Option<&mut T> {
        self.games
            .iter_mut()
            .find_map(|g| (&mut **g as &mut dyn Any).downcast_mut())
    }

    /// What the open game's screens see of the app.
    pub fn ctx(&self) -> Ctx<'_> {
        Ctx {
            size: self.size,
            online: true,
            room: self.online.room.as_ref(),
            account: self.online.account.as_ref(),
            notice: self.online.notice.as_deref(),
        }
    }

    pub fn resize(&mut self, size: Size) {
        self.size = size;
        for game in &mut self.games {
            game.resize(size);
        }
    }

    /// Advances what moves by itself, a game or the time spent in the
    /// queue, by `dt` of real time.
    pub fn tick(&mut self, dt: Duration) -> Vec<Effect> {
        match self.screen {
            Screen::Game => {
                let id = self.games[self.open].id();
                let fx = self.games[self.open].tick(dt);
                fx.into_iter().map(|fx| Effect::Game(id, fx)).collect()
            }
            Screen::Finding => {
                self.online.waited += dt;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// How soon the open game's screens need [`Self::tick`]; `None` while
    /// nothing moves by itself.
    pub fn next_wake(&self) -> Option<Duration> {
        match self.screen {
            Screen::Game => self.ui().next_wake(),
            _ => None,
        }
    }

    /// Whether the player is in a game on the open game's own screens.
    pub fn playing_alone(&self) -> bool {
        self.screen == Screen::Game && self.ui().playing()
    }

    /// What every game's progress so far needs kept, as the app closes.
    pub fn unfinished(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        for game in &mut self.games {
            let id = game.id();
            effects.extend(game.quit().into_iter().map(|fx| Effect::Game(id, fx)));
        }
        effects
    }

    /// The private room this player would make, as an invite to a friend
    /// does: the open game, with its settings.
    pub fn room_to_make(&self) -> (GameId, Vec<u8>) {
        (self.ui().id(), self.ui().room_settings())
    }

    pub fn handle(&mut self, key: Key) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.handle_key(key, &mut effects);
        effects
    }

    fn handle_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        // Games read keys as typed; the app's own screens only care about
        // case in what's typed into a field.
        let raw = key;
        let typing = matches!(self.screen, Screen::Input(_) | Screen::Form(_));
        let key = if typing { key } else { key.lower() };
        if key == Key::CtrlC {
            effects.extend(self.unfinished());
            effects.push(Effect::Quit);
            return;
        }
        if self.invite_key(key, effects) {
            return;
        }
        match self.screen {
            Screen::Picker { selected } => self.picker_key(selected, key, effects),
            Screen::Home { selected } => self.home_key(selected, key, effects),
            Screen::Input(field) => self.input_key(field, key, effects),
            Screen::Finding => self.finding_key(key, effects),
            // Nothing to do until the room answers.
            Screen::Joining { .. } | Screen::JoiningToWatch => {}
            Screen::Room => self.room_key(raw, effects),
            Screen::Form(kind) => self.form_key(kind, key, effects),
            Screen::Account { selected } => self.account_key(selected, key, effects),
            Screen::Friends { selected } => self.friends_key(selected, key, effects),
            Screen::Leaderboard { category } => self.leaderboard_keys(category, key, effects),
            Screen::Game => self.game_key(raw, effects),
        }
    }

    /// Carries out what `games[i]` made of a key.
    fn follow(&mut self, i: usize, reply: Reply, effects: &mut Vec<Effect>) {
        let id = self.games[i].id();
        effects.extend(reply.fx.into_iter().map(|fx| Effect::Game(id, fx)));
        match reply.next {
            Next::Stay => {}
            Next::Game => self.screen = Screen::Game,
            Next::Home => self.go_home(self.game_item, None),
            Next::Room if self.in_room() => self.screen = Screen::Room,
            Next::Room => self.go_home(HomeItem::QuickPlay, None),
            Next::Queue(pool) => self.queue(id, pool, effects),
        }
        if reply.notice.is_some() {
            self.online.notice = reply.notice;
        }
    }

    fn game_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        self.online.notice = None;
        let ctx = Ctx {
            size: self.size,
            online: true,
            room: self.online.room.as_ref(),
            account: self.online.account.as_ref(),
            notice: None,
        };
        let reply = self.games[self.open].key(key, &ctx);
        self.follow(self.open, reply, effects);
    }

    /// The home menu's entries: the open game's own first, then rooms,
    /// sign-up or friends, and the rest. New entries go near the end, so
    /// the old ones keep their places.
    pub fn home_items(&self) -> Vec<HomeItem> {
        let own = self.ui().home_items().len();
        let mut items: Vec<HomeItem> = (0..own).map(HomeItem::Game).collect();
        items.extend([
            HomeItem::QuickPlay,
            HomeItem::CreateRoom,
            HomeItem::JoinCode,
        ]);
        if self.online.account.is_some() {
            items.extend([HomeItem::Friends, HomeItem::Account]);
        } else {
            items.extend([HomeItem::Nickname, HomeItem::SignUp, HomeItem::SignIn]);
        }
        if self.games.len() > 1 {
            items.push(HomeItem::Games);
        }
        items.extend([HomeItem::Leaderboard, HomeItem::Quit]);
        items
    }

    /// What a home entry says.
    pub fn label(&self, item: HomeItem) -> &'static str {
        match item {
            HomeItem::Game(n) => self.ui().home_items().get(n).copied().unwrap_or(""),
            item => item.label(),
        }
    }

    pub fn go_home(&mut self, at: HomeItem, notice: Option<String>) {
        let selected = self.home_items().iter().position(|i| *i == at).unwrap_or(0);
        self.screen = Screen::Home { selected };
        self.online.notice = notice;
        self.online.confirm_small = false;
    }

    /// The game picker, at the open game.
    fn pick_game(&mut self) {
        self.online.notice = None;
        self.screen = Screen::Picker {
            selected: self.open,
        };
    }

    fn picker_key(&mut self, selected: usize, key: Key, effects: &mut Vec<Effect>) {
        if let Some(selected) = key.step(selected, self.games.len()) {
            self.screen = Screen::Picker { selected };
            return;
        }
        match key {
            Key::Enter | Key::Char(' ') => {
                self.switch_to(selected.min(self.games.len() - 1));
                self.screen = Screen::Home { selected: 0 };
                self.online.notice = None;
            }
            Key::Esc | Key::Char('q') if self.opened => self.go_home(HomeItem::Games, None),
            Key::Esc | Key::Char('q') => effects.push(Effect::Quit),
            _ => {}
        }
    }

    /// Whether the player is in the middle of playing, so an invite waits.
    fn busy_playing(&self) -> bool {
        self.playing_alone()
            || (self.screen == Screen::Room
                && self
                    .online
                    .room
                    .as_ref()
                    .is_some_and(|room| self.room_ui(room).in_play(room)))
    }

    /// The screens of `room`'s game, which is the open one.
    fn room_ui(&self, room: &InRoom) -> &dyn GameUi {
        let i = self.index_of(room.view.game()).unwrap_or(self.open);
        &*self.games[i]
    }

    /// Whether an invite is showing, so Enter and Esc answer it.
    pub fn invite_showing(&self) -> bool {
        self.online.invite.is_some() && !self.busy_playing()
    }

    /// Enter accepts a showing invite and Esc dismisses it. Returns whether
    /// the key was used.
    fn invite_key(&mut self, key: Key, effects: &mut Vec<Effect>) -> bool {
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
                self.online.invite = None;
                self.screen = Screen::Joining { by_code: false };
                true
            }
            Key::Esc => {
                effects.push(Effect::DismissInvite);
                self.online.invite = None;
                true
            }
            _ => false,
        }
    }

    fn home_key(&mut self, selected: usize, key: Key, effects: &mut Vec<Effect>) {
        let items = self.home_items();
        let had_notice = self.online.notice.take().is_some();
        if let Some(selected) = key.step(selected, items.len()) {
            self.online.confirm_small = false;
            self.screen = Screen::Home { selected };
            return;
        }
        match key {
            Key::Enter | Key::Char(' ') => {
                let item = items[selected.min(items.len() - 1)];
                self.choose(item, effects);
            }
            Key::Char('g') if self.games.len() > 1 => self.pick_game(),
            Key::Esc | Key::Char('q') if !had_notice => effects.push(Effect::Quit),
            _ => {}
        }
    }

    fn choose(&mut self, item: HomeItem, effects: &mut Vec<Effect>) {
        let game = self.ui().id();
        match item {
            HomeItem::Game(n) => {
                self.game_item = item;
                self.games[self.open].open(n);
                self.screen = Screen::Game;
            }
            HomeItem::QuickPlay => {
                let online = &mut self.online;
                let quick = match self.games[self.open].quick_play(self.size) {
                    Ok(QuickStart::Queue(quick)) => quick,
                    Ok(QuickStart::Choose) => {
                        self.game_item = item;
                        self.screen = Screen::Game;
                        return;
                    }
                    Err(why) => {
                        online.notice = Some(why);
                        return;
                    }
                };
                if let Some(warning) = quick.too_small
                    && !online.confirm_small
                {
                    online.notice = Some(warning);
                    online.confirm_small = true;
                    return;
                }
                self.queue(game, quick.pool, effects);
            }
            HomeItem::CreateRoom => {
                self.screen = Screen::Joining { by_code: false };
                effects.push(Effect::CreateRoom {
                    game,
                    settings: self.ui().room_settings(),
                });
            }
            HomeItem::JoinCode => self.start_input(Field::Code, String::new()),
            HomeItem::Nickname => {
                let nick = self.online.name.trim_start_matches('~').to_string();
                self.start_input(Field::Nickname, nick);
            }
            HomeItem::Friends => {
                self.online.friends_from_room = false;
                self.online.confirm_remove = None;
                self.screen = Screen::Friends { selected: 0 };
                effects.push(Effect::LoadFriends);
            }
            HomeItem::Account => {
                self.screen = Screen::Account { selected: 0 };
                effects.push(Effect::LoadAccount);
            }
            HomeItem::SignUp => self.start_form(FormKind::SignUp),
            HomeItem::SignIn => self.start_form(FormKind::SignIn),
            HomeItem::Games => self.pick_game(),
            HomeItem::Leaderboard => self.show_leaderboard(0, effects),
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

    fn input_key(&mut self, field: Field, key: Key, effects: &mut Vec<Effect>) {
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

    /// Into quick play, waiting in `game`'s `pool`.
    fn queue(&mut self, game: GameId, pool: &'static str, effects: &mut Vec<Effect>) {
        let online = &mut self.online;
        online.confirm_small = false;
        online.waited = Duration::ZERO;
        online.pool = Some(pool);
        self.screen = Screen::Finding;
        effects.push(Effect::QuickPlay { game, pool });
    }

    fn finding_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        match key {
            Key::Esc | Key::Char('q') => {
                effects.push(Effect::LeaveQueue);
                self.go_home(HomeItem::QuickPlay, None);
            }
            // The match comes back like any other.
            Key::Char('b') if self.bots_offered() => effects.push(Effect::PlayBots),
            _ => {}
        }
    }

    /// Whether quick play has waited long enough to offer bots instead.
    pub fn bots_offered(&self) -> bool {
        self.ui().bots() && self.online.waited >= BOTS_OFFERED_AFTER
    }

    fn room_key(&mut self, key: Key, effects: &mut Vec<Effect>) {
        let signed_in = self.online.account.is_some();
        self.online.notice = None;
        let Some(room) = self.online.room.as_ref() else {
            return;
        };
        let i = self.index_of(room.view.game()).unwrap_or(self.open);
        if let Some(reply) = self.games[i].room_key(key, room) {
            return self.follow(i, reply, effects);
        }
        let key = key.lower();
        let ui = &*self.games[i];
        let playing = ui.in_play(room);
        let bots = ui.bots();
        let shell = room.shell();
        let (hosting, kind) = (room.hosting(), shell.kind);
        let full = shell.seats.len() >= shell.max_players;
        let has_bot = shell.seats.iter().any(|s| is_bot(s.who.id));
        let (confirm_leave, watching) = (room.confirm_leave, room.watching.is_some());
        let phase = Phase::of(&shell.phase);
        // Watching: Esc goes back to the friends list.
        if watching {
            if matches!(key, Key::Esc | Key::Char('q')) {
                effects.extend([Effect::LeaveRoom, Effect::LoadFriends]);
                self.online.room = None;
                self.online.friends_from_room = false;
                self.screen = Screen::Friends { selected: 0 };
            }
            return;
        }
        let online = &mut self.online;
        let leave = match phase {
            Phase::Lobby => match key {
                Key::Enter | Key::Char(' ' | 'r') => {
                    effects.push(Effect::ToggleReady);
                    false
                }
                Key::Char('b') if hosting && bots => {
                    if full {
                        online.notice = Some("The room is full.".to_string());
                    } else {
                        effects.push(Effect::AddBot);
                    }
                    false
                }
                Key::Char('x') if hosting && bots => {
                    if has_bot {
                        effects.push(Effect::RemoveBot);
                    }
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
            Phase::Round => {
                let confirming = |on: bool, online: &mut Online| {
                    if let Some(room) = online.room.as_mut() {
                        room.confirm_leave = on;
                    }
                };
                match key {
                    Key::Char('y') if confirm_leave => true,
                    Key::Char('n') | Key::Esc if confirm_leave => {
                        confirming(false, online);
                        false
                    }
                    Key::Esc | Key::Char('q') if playing => {
                        confirming(true, online);
                        false
                    }
                    Key::Esc | Key::Char('q') => true,
                    _ => false,
                }
            }
            Phase::Results => match key {
                Key::Enter | Key::Char(' ') if kind == RoomKind::Match => {
                    effects.push(Effect::LeaveRoom);
                    self.leave_room();
                    self.choose(HomeItem::QuickPlay, effects);
                    return;
                }
                Key::Esc | Key::Char('q') => true,
                _ => false,
            },
            Phase::Closed => true,
        };
        if leave {
            effects.push(Effect::LeaveRoom);
            self.leave_room();
        }
    }

    fn leave_room(&mut self) {
        let at = match self.online.room.take().map(|r| r.shell().kind) {
            Some(RoomKind::Private) => HomeItem::CreateRoom,
            _ => HomeItem::QuickPlay,
        };
        self.go_home(at, None);
    }

    fn form_key(&mut self, kind: FormKind, key: Key, effects: &mut Vec<Effect>) {
        let rows = kind.rows(self.online.has_key);
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

    fn account_key(&mut self, selected: usize, key: Key, effects: &mut Vec<Effect>) {
        let online = &mut self.online;
        online.notice = None;
        let rows = online.account_rows();
        if let Some(selected) = key.step(selected, rows.len()) {
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

    /// The open game's rating number `category`, in [`GameUi::ratings`].
    pub fn rating_key(&self, category: usize) -> RatingKey {
        let ratings = self.ui().ratings();
        RatingKey {
            game: self.ui().id(),
            category: ratings[category.min(ratings.len() - 1)].id,
        }
    }

    /// The account's rating that the home menu shows: the open game's
    /// first.
    pub fn main_rating(&self) -> Option<&ShownRating> {
        self.online.account.as_ref()?.rating(self.rating_key(0))
    }

    fn show_leaderboard(&mut self, category: usize, effects: &mut Vec<Effect>) {
        // Fresh every time, so it never shows old places.
        self.online.leaderboard = None;
        self.screen = Screen::Leaderboard { category };
        effects.push(Effect::LoadLeaderboard(self.rating_key(category)));
    }

    fn leaderboard_keys(&mut self, category: usize, key: Key, effects: &mut Vec<Effect>) {
        let len = self.ui().ratings().len();
        match key {
            Key::Esc | Key::Enter | Key::Char('q') => self.go_home(HomeItem::Leaderboard, None),
            // The game's other ratings, when it has some.
            Key::Left | Key::Char('a' | 'h') if len > 1 => {
                self.show_leaderboard(category.checked_sub(1).unwrap_or(len - 1), effects);
            }
            Key::Right | Key::Char('d' | 'l') if len > 1 => {
                self.show_leaderboard((category + 1) % len, effects);
            }
            _ => {}
        }
    }

    fn friends_key(&mut self, selected: usize, key: Key, effects: &mut Vec<Effect>) {
        let online = &mut self.online;
        online.notice = None;
        let rows = online.friends.rows();
        if let Some(selected) = key.step(selected, rows.len()) {
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
            (FriendsRow::Friend(i), Key::Char('v')) => {
                let friend = &online.friends.friends[i];
                if friend.watch {
                    if online.room.is_some() {
                        effects.push(Effect::LeaveRoom);
                        online.room = None;
                    }
                    effects.push(Effect::Watch(friend.name.clone()));
                    self.screen = Screen::JoiningToWatch;
                } else {
                    online.notice = Some(format!("{} isn't playing a game to watch.", friend.name));
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
        self.online.name = name;
    }

    /// Something to tell the player on the screen they're on.
    pub fn notify(&mut self, notice: String) {
        self.online.notice = Some(notice);
    }

    pub fn set_queue_len(&mut self, n: usize) {
        self.online.queue_len = n;
    }

    pub fn set_region(&mut self, region: QueueRegion) {
        self.online.region = Some(region);
    }

    /// What was kept for the player in `game`, for its screens.
    pub fn restore(&mut self, game: GameId, saved: Saved) {
        if let Some(i) = self.index_of(game) {
            self.games[i].restore(saved);
        }
    }

    /// What to keep for the player in `game` now.
    pub fn saved(&self, game: GameId) -> Option<Saved> {
        Some(self.games[self.index_of(game)?].saved())
    }

    /// Whether the player is waiting in the queue, so a match is welcome.
    pub fn wants_match(&self) -> bool {
        self.screen == Screen::Finding
    }

    pub fn in_room(&self) -> bool {
        self.online.room.is_some()
    }

    /// The player is now seated in a room, of whichever game: that game
    /// opens. `false` if the app has no screens for it.
    pub fn enter_room(&mut self, me: PlayerId, view: Arc<dyn AnyView>) -> bool {
        let Some(i) = self.index_of(view.game()) else {
            return false;
        };
        self.switch_to(i);
        self.online.notice = None;
        self.online.room = Some(InRoom {
            me,
            view,
            confirm_leave: false,
            watching: None,
        });
        self.screen = Screen::Room;
        true
    }

    /// The player is back in a game they're still playing, from a
    /// connection that dropped: it opens, whatever was showing.
    pub fn rejoin_room(&mut self, me: PlayerId, view: Arc<dyn AnyView>) -> bool {
        if !self.enter_room(me, view) {
            return false;
        }
        self.online.notice = Some("Welcome back: your game went on.".to_string());
        true
    }

    /// This player now watches `friend` play, in a room with no seat for
    /// them. `false` if the app has no screens for its game.
    pub fn watch_room(&mut self, me: PlayerId, friend: String, view: Arc<dyn AnyView>) -> bool {
        if !self.enter_room(me, view) {
            return false;
        }
        if let Some(room) = self.online.room.as_mut() {
            room.watching = Some(friend);
        }
        true
    }

    /// Watching a friend didn't work: back to the friends list, saying why.
    pub fn watch_failed(&mut self, why: String) {
        if self.screen == Screen::JoiningToWatch {
            self.screen = Screen::Friends { selected: 0 };
            self.notify(why);
        }
    }

    /// The room changed.
    pub fn update_room(&mut self, view: Arc<dyn AnyView>) {
        if let PhaseView::Closed { reason } = &view.shell().phase {
            return self.room_closed(Some(reason.clone()));
        }
        let Some(room) = self.online.room.as_mut() else {
            return;
        };
        // A room plays one game all its life.
        if view.game() != room.view.game() {
            return;
        }
        let lobby = matches!(view.shell().phase, PhaseView::Waiting { .. });
        let old = std::mem::replace(&mut room.view, view);
        let i = self.games.iter().position(|g| g.id() == room.view.game());
        let ui = &*self.games[i.unwrap_or(self.open)];
        if !ui.in_play(room) {
            room.confirm_leave = false;
        }
        // Everyone but the host hears what the host changed.
        if lobby
            && !room.hosting()
            && let Some(note) = ui.settings_note(&*old, room)
        {
            self.online.notice = Some(note);
        }
        // A round starting pulls the player back from the friends list and
        // the game's own screens, such as the rules.
        if !lobby
            && matches!(
                self.screen,
                Screen::Friends { .. } | Screen::Input(_) | Screen::Game
            )
        {
            self.screen = Screen::Room;
        }
    }

    /// The room is gone; `reason` says why, if the room said.
    pub fn room_closed(&mut self, reason: Option<String>) {
        if let Some(room) = &self.online.room {
            let watching = room.watching.is_some();
            self.leave_room();
            self.online.notice = reason.or_else(|| watching.then(|| "That game is over.".into()));
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
        if let Some(p) = &profile {
            self.online.name = p.username.clone();
        }
        self.online.account = profile;
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

    /// Signed in. The account's settings and records replace the guest's
    /// through [`Self::restore`].
    pub fn signed_in(&mut self, profile: Profile) {
        let notice = format!("Signed in as {}.", profile.username);
        self.form = Form::default();
        self.set_profile(Some(profile));
        self.go_home(HomeItem::QuickPlay, Some(notice));
    }

    /// Back to being the guest `name`. Their settings and records come
    /// back through [`Self::restore`].
    pub fn signed_out(&mut self, name: String) {
        let online = &mut self.online;
        online.account = None;
        online.name = name;
        online.keys.clear();
        online.friends = FriendsView::default();
        online.requests = 0;
        online.invite = None;
        self.go_home(HomeItem::SignIn, Some("Signed out.".to_string()));
    }

    pub fn password_changed(&mut self) {
        self.form = Form::default();
        self.screen = Screen::Account { selected: 0 };
        self.notify("Password changed.".to_string());
    }

    pub fn set_keys(&mut self, keys: Vec<KeyRow>) {
        self.online.keys = keys;
        if let Screen::Account { selected } = self.screen {
            let last = self.online.account_rows().len() - 1;
            self.screen = Screen::Account {
                selected: selected.min(last),
            };
        }
    }

    pub fn set_friends(&mut self, friends: FriendsView) {
        let online = &mut self.online;
        online.requests = friends.incoming.len();
        online.friends = friends;
        if let Screen::Friends { selected } = self.screen {
            let last = online.friends.rows().len() - 1;
            self.screen = Screen::Friends {
                selected: selected.min(last),
            };
        }
    }

    /// The leaderboard loaded; one for another rating than the one now
    /// showing is too late, and dropped.
    pub fn set_leaderboard(&mut self, view: LeaderboardView) {
        if let Screen::Leaderboard { category } = self.screen
            && view.key != self.rating_key(category)
        {
            return;
        }
        self.online.leaderboard = Some(view);
    }

    /// `from` invites this player to their room, of `game`.
    pub fn invited(&mut self, from: String, game: GameId) {
        self.online.invite = Some(Invite { from, game });
    }

    /// The invite expired or its room went away.
    pub fn invite_gone(&mut self) {
        self.online.invite = None;
    }

    pub fn friend_requested(&mut self, from: &str) {
        let online = &mut self.online;
        online.requests += 1;
        online.notice = Some(format!(
            "{from} wants to be friends. Open Friends to answer."
        ));
    }

    /// A rated round moved this player's rating `key`, which may still
    /// be `provisional`.
    pub fn rated(&mut self, key: RatingKey, rating: u32, provisional: bool, won: bool) {
        let Some(profile) = self.online.account.as_mut() else {
            return;
        };
        match profile.ratings.iter_mut().find(|r| r.key == key) {
            Some(shown) => {
                shown.rating = rating;
                shown.provisional = provisional;
                shown.games += 1;
                shown.wins += u32::from(won);
            }
            None => profile.ratings.push(ShownRating {
                key,
                rating,
                games: 1,
                wins: u32::from(won),
                provisional,
            }),
        }
    }
}
