//! Snake as the app shows it: single player on its own screens, and its
//! part of rooms. The local game drives the same [`SnakeUi`] on its own.

use std::time::Duration;

use game_api::codec::encode;
use game_api::{AnyView, PlayerId};
use ratatui::Frame;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::text::Span;
use shell_tui::online::{Category, InRoom};
use shell_tui::{Ctx, GameFx, GameUi, Key, Next, QuickPlay, QuickStart, Reply, Saved};
use snake_core::config::{GameConfig, Rules, TICK_MS_MAX, TICK_MS_MIN, TICK_MS_STEP};
use snake_core::geom::Direction;
use snake_core::records::Records;
use snake_core::round::{
    PhaseView, QUICK_POOL, QUICK_RATING, RacerView, RoomView, SeatView, Snake, max_players_for,
    quick_rules,
};
use snake_core::settings::{SETTINGS, room_settings};
use snake_core::solo::SoloGame;

use crate::ui;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnakeScreen {
    /// Single-player settings. The local game starts here.
    Menu,
    Playing,
    Paused {
        selected: usize,
    },
    GameOver {
        new_record: bool,
    },
    /// A private room's rules, at [`room_settings`] entry `selected`. The
    /// host changes them.
    RoomRules {
        selected: usize,
    },
}

pub struct SnakeUi {
    pub cfg: GameConfig,
    /// The setting picked on the settings screen, by its place in
    /// [`SETTINGS`].
    pub selected: usize,
    pub game: Option<SoloGame>,
    pub records: Records,
    /// The best on the rules being played, as the game started.
    pub record: u32,
    pub screen: SnakeScreen,
    /// Set by the first press of the reset key; a second press resets.
    pub confirm_reset: bool,
    /// The terminal the game is drawn on. Set it with [`GameUi::resize`].
    pub size: Size,
}

impl Default for SnakeUi {
    fn default() -> Self {
        Self::new(GameConfig::default(), Records::default())
    }
}

/// A room's view, if it's Snake's.
pub(crate) fn snake_view(room: &InRoom) -> Option<&RoomView> {
    room.view.of::<Snake>()
}

/// The snake `seat` has this round.
pub(crate) fn racer<'a>(view: &'a RoomView, seat: &SeatView) -> Option<&'a RacerView> {
    seat.slot
        .and_then(|slot| view.game.racers.get(slot as usize))
}

impl SnakeUi {
    /// Snake with these settings and records, at the settings screen.
    pub fn new(cfg: GameConfig, records: Records) -> Self {
        Self {
            cfg,
            selected: 0,
            game: None,
            records,
            record: 0,
            screen: SnakeScreen::Menu,
            confirm_reset: false,
            size: Size::new(0, 0),
        }
    }

    /// How long the local game may wait for a key before ticking.
    pub fn poll_interval(&self) -> Duration {
        if self.screen == SnakeScreen::Playing {
            Duration::from_millis(15)
        } else {
            Duration::from_millis(100)
        }
    }

    /// The snake must never move while its board isn't on screen.
    fn pause_if_hidden(&mut self) {
        if self.screen == SnakeScreen::Playing
            && self
                .game
                .as_ref()
                .is_some_and(|g| !ui::board_fits(g.world.width, g.world.height, self.size))
        {
            self.screen = SnakeScreen::Paused { selected: 0 };
        }
    }

    fn start_game(&mut self, fx: &mut Vec<GameFx>) {
        if let Ok(game) = SoloGame::new(self.cfg.rules.clone()) {
            fx.push(GameFx::SavePrefs(self.cfg.to_saved_text()));
            self.record = self.records.get(&game.rules().record_key());
            self.game = Some(game);
            self.screen = SnakeScreen::Playing;
            self.pause_if_hidden();
        }
    }

    fn save_record(&mut self, fx: &mut Vec<GameFx>) -> bool {
        let Some(game) = &self.game else { return false };
        let len = game.len() as u32;
        let key = game.rules().record_key();
        let new_record = self.records.update(&key, len);
        if new_record {
            fx.push(GameFx::Record { key, best: len });
        }
        self.record = self.record.max(len);
        new_record
    }

    fn quit_to_menu(&mut self, fx: &mut Vec<GameFx>) {
        self.save_record(fx);
        self.game = None;
        self.screen = SnakeScreen::Menu;
    }

    fn menu_key(&mut self, key: Key, reply: &mut Reply) {
        let confirm_reset = std::mem::take(&mut self.confirm_reset);
        match key {
            Key::Esc | Key::Char('q') => reply.next = Next::Home,
            Key::Up | Key::Down | Key::Char('w' | 'k' | 's' | 'j') => {
                self.selected = key.step(self.selected, SETTINGS.len()).unwrap_or(0);
            }
            Key::Left | Key::Char('a' | 'h') => self.cfg.adjust(self.selected, -1),
            Key::Right | Key::Char('d' | 'l') => self.cfg.adjust(self.selected, 1),
            Key::Char('r') if confirm_reset => self.cfg = GameConfig::default(),
            Key::Char('r') => self.confirm_reset = true,
            Key::Enter => self.start_game(&mut reply.fx),
            _ => {}
        }
    }

    /// The room's rules, in the lobby: the host changes them with ←→.
    fn room_rules_key(&mut self, selected: usize, key: Key, room: Option<&InRoom>) -> Reply {
        let Some((room, view)) = room.and_then(|r| Some((r, snake_view(r)?))) else {
            return Reply::to(Next::Room);
        };
        let items = room_settings();
        if let Some(selected) = key.step(selected, items.len()) {
            self.screen = SnakeScreen::RoomRules { selected };
            return Reply::default();
        }
        match key {
            Key::Left | Key::Right | Key::Char('a' | 'h' | 'd' | 'l') if room.hosting() => {
                let delta = if matches!(key, Key::Left | Key::Char('a' | 'h')) {
                    -1
                } else {
                    1
                };
                let mut cfg = GameConfig {
                    rules: view.settings.clone(),
                    ..GameConfig::default()
                };
                cfg.adjust(items[selected.min(items.len() - 1)], delta);
                if cfg.rules == view.settings {
                    return Reply::default();
                }
                // The room would refuse these too, without a word.
                let players = view.shell.seats.len();
                match max_players_for(&cfg.rules) {
                    Some(fit) if fit >= players => Reply::fx(GameFx::Settings(encode(&cfg.rules))),
                    _ => Reply::notice(format!("{players} snakes don't fit on that board.")),
                }
            }
            Key::Esc | Key::Enter | Key::Char('q' | 'e') => Reply::to(Next::Room),
            _ => Reply::default(),
        }
    }
}

pub(crate) fn direction(key: Key) -> Option<Direction> {
    match key {
        Key::Up | Key::Char('w') => Some(Direction::Up),
        Key::Down | Key::Char('s') => Some(Direction::Down),
        Key::Left | Key::Char('a') => Some(Direction::Left),
        Key::Right | Key::Char('d') => Some(Direction::Right),
        _ => None,
    }
}

/// What the host changed, going from the rules `old` to those in `room`.
fn rules_changed(old: &Rules, room: &RoomView) -> String {
    let host = room
        .shell
        .host()
        .map_or("The host", |h| h.who.name.as_str());
    let value = |rules: &Rules, i: usize| {
        GameConfig {
            rules: rules.clone(),
            ..GameConfig::default()
        }
        .value_str(i)
    };
    let changed: Vec<usize> = room_settings()
        .into_iter()
        .filter(|&i| value(old, i) != value(&room.settings, i))
        .collect();
    match changed[..] {
        [i] => format!(
            "{host} set {} to {}.",
            SETTINGS[i].label,
            value(&room.settings, i)
        ),
        _ => format!("{host} changed the rules."),
    }
}

impl GameUi for SnakeUi {
    fn id(&self) -> game_api::GameId {
        game_api::GameId::Snake
    }

    fn title(&self) -> &'static str {
        "Snake"
    }

    fn logo(&self) -> (&'static [&'static str], Color) {
        (&ui::LOGO, Color::LightGreen)
    }

    fn home_items(&self) -> &'static [&'static str] {
        &["Single player"]
    }

    fn open(&mut self, _: usize) {
        self.screen = SnakeScreen::Menu;
    }

    fn room_note(&self) -> Option<String> {
        Some("your settings".to_string())
    }

    fn room_settings(&self) -> Vec<u8> {
        encode(&self.cfg.rules)
    }

    fn quick_play(&mut self, size: Size) -> Result<QuickStart, String> {
        let rules = quick_rules();
        let too_small = (!ui::room_fits(rules.width, rules.height, size)).then(|| {
            let (w, h) = ui::room_size(rules.width, rules.height);
            format!(
                "Your terminal is {}×{}; matches need {w}×{h} to show the whole board. \
                 Press Enter again to play anyway.",
                size.width, size.height
            )
        });
        Ok(QuickStart::Queue(QuickPlay {
            pool: QUICK_POOL,
            too_small,
        }))
    }

    fn ratings(&self) -> &'static [Category] {
        &[Category {
            id: QUICK_RATING.category,
            label: "Quick play",
        }]
    }

    fn bots(&self) -> bool {
        true
    }

    fn key(&mut self, key: Key, ctx: &Ctx) -> Reply {
        // Snake's screens type nothing, so case never matters.
        let key = key.lower();
        let mut reply = Reply::default();
        match self.screen {
            SnakeScreen::Menu => self.menu_key(key, &mut reply),
            SnakeScreen::Playing => {
                if matches!(key, Key::Esc | Key::Char('p')) {
                    self.screen = SnakeScreen::Paused { selected: 0 };
                } else if let (Some(dir), Some(game)) = (direction(key), &mut self.game) {
                    game.turn(dir);
                }
            }
            SnakeScreen::Paused { selected } => match key {
                Key::Esc | Key::Char('r' | 'p') => self.screen = SnakeScreen::Playing,
                Key::Up | Key::Down | Key::Char('w' | 's' | 'k' | 'j') => {
                    self.screen = SnakeScreen::Paused {
                        selected: 1 - selected,
                    };
                }
                Key::Enter if selected == 0 => self.screen = SnakeScreen::Playing,
                Key::Enter | Key::Char('q') => self.quit_to_menu(&mut reply.fx),
                _ => {}
            },
            SnakeScreen::GameOver { .. } => match key {
                Key::Enter | Key::Esc | Key::Char(' ') => {
                    self.game = None;
                    self.screen = SnakeScreen::Menu;
                }
                Key::Char('r') => self.start_game(&mut reply.fx),
                Key::Char('q') => {
                    self.game = None;
                    self.screen = SnakeScreen::Menu;
                    reply.next = Next::Home;
                }
                _ => {}
            },
            SnakeScreen::RoomRules { selected } => {
                reply = self.room_rules_key(selected, key, ctx.room);
            }
        }
        self.pause_if_hidden();
        reply
    }

    fn tick(&mut self, dt: Duration) -> Vec<GameFx> {
        let mut fx = Vec::new();
        if self.screen == SnakeScreen::Playing {
            if let Some(game) = &mut self.game {
                game.advance(dt);
            }
            if self.game.as_ref().is_some_and(|g| g.over.is_some()) {
                let new_record = self.save_record(&mut fx);
                self.screen = SnakeScreen::GameOver { new_record };
            }
        }
        fx
    }

    fn next_wake(&self) -> Option<Duration> {
        match (&self.screen, &self.game) {
            // Frozen until the first key; nothing to wake for.
            (SnakeScreen::Playing, Some(game)) if game.started => {
                Some(game.until_next_step().unwrap_or_default())
            }
            _ => None,
        }
    }

    fn resize(&mut self, size: Size) {
        self.size = size;
        self.pause_if_hidden();
    }

    fn playing(&self) -> bool {
        self.screen == SnakeScreen::Playing
    }

    fn draw(&self, frame: &mut Frame, ctx: &Ctx) {
        ui::draw(frame, self, ctx);
    }

    fn in_play(&self, room: &InRoom) -> bool {
        let Some(view) = snake_view(room) else {
            return false;
        };
        matches!(
            view.shell.phase,
            PhaseView::Countdown { .. } | PhaseView::Running { .. }
        ) && room
            .my_seat()
            .and_then(|s| racer(view, s))
            .is_some_and(|r| r.alive)
    }

    fn room_key(&mut self, key: Key, room: &InRoom) -> Option<Reply> {
        let key = key.lower();
        let view = snake_view(room)?;
        match &view.shell.phase {
            PhaseView::Waiting { .. } => match key {
                // The host makes the next round faster or slower, a step
                // at a time on the settings' grid. The room decides; its
                // next view shows the speed.
                Key::Left | Key::Right | Key::Char('a' | 'h' | 'd' | 'l') if room.hosting() => {
                    let now = view.settings.tick_ms;
                    let next = if matches!(key, Key::Left | Key::Char('a' | 'h')) {
                        now.saturating_sub(1) / TICK_MS_STEP * TICK_MS_STEP
                    } else {
                        (now / TICK_MS_STEP + 1) * TICK_MS_STEP
                    }
                    .clamp(TICK_MS_MIN, TICK_MS_MAX);
                    if next == now {
                        return Some(Reply::default());
                    }
                    let rules = Rules {
                        tick_ms: next,
                        ..view.settings.clone()
                    };
                    Some(Reply::fx(GameFx::Settings(encode(&rules))))
                }
                // Everyone can read the rules; the host changes them there.
                Key::Char('e') => {
                    self.screen = SnakeScreen::RoomRules { selected: 0 };
                    Some(Reply::to(Next::Game))
                }
                _ => None,
            },
            PhaseView::Countdown { .. } | PhaseView::Running { .. } if self.in_play(room) => {
                direction(key).map(|dir| Reply::fx(GameFx::Play(encode(&dir))))
            }
            _ => None,
        }
    }

    fn settings_note(&self, old: &dyn AnyView, room: &InRoom) -> Option<String> {
        let (old, now) = (old.of::<Snake>()?, snake_view(room)?);
        (old.settings != now.settings).then(|| rules_changed(&old.settings, now))
    }

    fn lobby_line(&self, room: &InRoom) -> Vec<Span<'static>> {
        let Some(view) = snake_view(room) else {
            return Vec::new();
        };
        let rules = &view.settings;
        let dim = shell_tui::ui::dim();
        // The host can change the speed, so theirs stands out, with arrows.
        let tick = format!("{} ms ticks", rules.tick_ms);
        let tick = if room.hosting() {
            Span::styled(
                format!("◂ {tick} ▸"),
                ratatui::style::Style::default().fg(Color::LightYellow),
            )
        } else {
            Span::styled(tick, dim)
        };
        vec![
            Span::styled(format!("{}×{} board · ", rules.width, rules.height), dim),
            tick,
        ]
    }

    fn lobby_keys(&self) -> Option<&'static str> {
        Some("e rules")
    }

    fn host_keys(&self) -> Option<&'static str> {
        Some("←→ speed")
    }

    fn draw_round(&self, frame: &mut Frame, room: &InRoom, ctx: &Ctx) {
        let _ = ctx;
        ui::draw_round(frame, self, room);
    }

    fn result_note(&self, room: &InRoom) -> Option<String> {
        snake_view(room)?
            .game
            .time_up
            .then(|| "Time's up".to_string())
    }

    fn score(&self, room: &InRoom, who: PlayerId) -> String {
        let length = snake_view(room)
            .and_then(|v| v.game.racer(who))
            .map_or(0, |r| r.length);
        length.to_string()
    }

    fn restore(&mut self, saved: Saved) {
        self.cfg = GameConfig::from_saved_text(&saved.prefs);
        self.records = saved.records;
    }

    fn saved(&self) -> Saved {
        Saved {
            prefs: self.cfg.to_saved_text(),
            records: self.records.clone(),
        }
    }

    fn quit(&mut self) -> Vec<GameFx> {
        let mut fx = Vec::new();
        self.save_record(&mut fx);
        fx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snake() -> SnakeUi {
        let mut ui = SnakeUi::default();
        ui.resize(Size::new(120, 40));
        ui
    }

    fn key(ui: &mut SnakeUi, key: Key) -> Reply {
        ui.key(key, &Ctx::alone(ui.size))
    }

    #[test]
    fn menu_d_adjusts_and_r_twice_resets() {
        let mut ui = snake();
        key(&mut ui, Key::Char('d'));
        key(&mut ui, Key::Char('D'));
        assert_eq!(ui.cfg.rules.width, 32);
        key(&mut ui, Key::Char('r'));
        assert!(ui.confirm_reset);
        key(&mut ui, Key::Char('x'));
        assert_eq!(ui.cfg.rules.width, 32, "any other key cancels the reset");
        key(&mut ui, Key::Char('r'));
        key(&mut ui, Key::Char('r'));
        assert_eq!(ui.cfg, GameConfig::default());
    }

    #[test]
    fn quitting_keeps_a_new_record() {
        let mut ui = snake();
        assert_eq!(ui.quit(), [], "nothing played yet");
        let reply = key(&mut ui, Key::Enter);
        assert_eq!(
            reply.fx,
            [GameFx::SavePrefs(GameConfig::default().to_saved_text())]
        );
        assert_eq!(ui.screen, SnakeScreen::Playing);
        // The first game on these rules is a new record, saved on the way
        // out.
        let len = ui.game.as_ref().unwrap().len() as u32;
        let key = GameConfig::default().rules.record_key();
        assert_eq!(ui.quit(), [GameFx::Record { key, best: len }]);
    }

    #[test]
    fn pauses_when_the_board_stops_fitting() {
        let mut ui = snake();
        key(&mut ui, Key::Enter);
        assert_eq!(ui.screen, SnakeScreen::Playing);
        ui.resize(Size::new(40, 15));
        assert_eq!(ui.screen, SnakeScreen::Paused { selected: 0 });
        key(&mut ui, Key::Esc);
        assert_eq!(
            ui.screen,
            SnakeScreen::Paused { selected: 0 },
            "can't resume yet"
        );
        ui.resize(Size::new(120, 40));
        key(&mut ui, Key::Esc);
        assert_eq!(ui.screen, SnakeScreen::Playing);
    }

    #[test]
    fn the_settings_and_a_finished_game_go_home_on_q() {
        let mut ui = snake();
        assert_eq!(key(&mut ui, Key::Char('q')).next, Next::Home);
        key(&mut ui, Key::Enter);
        ui.game.as_mut().unwrap().over = Some(snake_core::solo::GameOverReason::HitWall);
        ui.tick(Duration::from_millis(1));
        assert!(matches!(ui.screen, SnakeScreen::GameOver { .. }));
        assert_eq!(key(&mut ui, Key::Char('q')).next, Next::Home);
        assert_eq!(ui.screen, SnakeScreen::Menu);
    }
}
