//! A room's rounds, for any game: players gather, a countdown runs, the
//! game's [`Round`] plays, and the results show. A matched room plays one
//! round; a private room plays round after round.

use std::fmt;
use std::time::Duration;

use rand::SeedableRng;
use rand::rngs::Xoshiro256PlusPlus;

use crate::view::{PhaseView, RoomView, SeatView, ShellView};
use crate::{
    Entrant, Game, JoinError, MIN_PLAYERS, Placement, PlayerId, RoomKind, Round, RoundEnd, is_bot,
    winner,
};

/// Where every random choice in a room comes from.
pub type RoomRng = Xoshiro256PlusPlus;

pub struct RoomSettings<G: Game> {
    pub game: G::Settings,
    pub kind: RoomKind,
    pub max_players: usize,
    /// Private rooms: how long after the second player arrives the countdown
    /// starts by itself; `None` waits until everyone is ready.
    pub start_after: Option<Duration>,
    pub countdown: Duration,
    /// Private rooms: how long the results show before the next round.
    pub results_for: Duration,
}

impl<G: Game> RoomSettings<G> {
    /// `settings` as rooms play them; `None` when not even [`MIN_PLAYERS`]
    /// fit.
    pub fn new(settings: G::Settings, kind: RoomKind) -> Option<Self> {
        let game = G::for_rooms(settings);
        let max_players = G::seats(&game)?;
        Some(Self {
            game,
            kind,
            max_players,
            start_after: Some(Duration::from_secs(15)),
            countdown: Duration::from_secs(3),
            results_for: Duration::from_secs(8),
        })
    }
}

impl<G: Game> Clone for RoomSettings<G> {
    fn clone(&self) -> Self {
        Self {
            game: self.game.clone(),
            kind: self.kind,
            max_players: self.max_players,
            start_after: self.start_after,
            countdown: self.countdown,
            results_for: self.results_for,
        }
    }
}

impl<G: Game> fmt::Debug for RoomSettings<G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomSettings")
            .field("game", &self.game)
            .field("kind", &self.kind)
            .field("max_players", &self.max_players)
            .field("start_after", &self.start_after)
            .field("countdown", &self.countdown)
            .field("results_for", &self.results_for)
            .finish()
    }
}

impl<G: Game> PartialEq for RoomSettings<G> {
    fn eq(&self, other: &Self) -> bool {
        self.game == other.game
            && self.kind == other.kind
            && self.max_players == other.max_players
            && self.start_after == other.start_after
            && self.countdown == other.countdown
            && self.results_for == other.results_for
    }
}

/// Times are on the room's own clock.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Waiting { start_at: Option<Duration> },
    Countdown { go_at: Duration },
    Running,
    Results { next_at: Option<Duration> },
}

struct Seat {
    who: Entrant,
    ready: bool,
    /// This player's place in this round.
    slot: Option<u8>,
    wins: u32,
}

pub struct Room<G: Game> {
    pub settings: RoomSettings<G>,
    clock: Duration,
    phase: Phase,
    seats: Vec<Seat>,
    /// This round's players, by slot: kept when they leave, for the
    /// results.
    players: Vec<Entrant>,
    /// Their ids as the round began, by slot, which its results name them
    /// by: some may have come back as someone new since.
    started: Vec<PlayerId>,
    /// From the countdown until the next round's lobby.
    round: Option<G::Round>,
    placements: Vec<Placement>,
    rng: RoomRng,
    changed: bool,
    shown_secs: Option<u64>,
}

impl<G: Game> Room<G> {
    /// A private room, empty and waiting for players.
    pub fn new(settings: RoomSettings<G>, seed: u64) -> Self {
        Self {
            settings,
            clock: Duration::ZERO,
            phase: Phase::Waiting { start_at: None },
            seats: Vec::new(),
            players: Vec::new(),
            started: Vec::new(),
            round: None,
            placements: Vec::new(),
            rng: RoomRng::seed_from_u64(seed),
            changed: true,
            shown_secs: None,
        }
    }

    /// A matched room: these players, counting down at once.
    pub fn new_match(settings: RoomSettings<G>, seed: u64, players: Vec<Entrant>) -> Self {
        let mut room = Self::new(settings, seed);
        for who in players.into_iter().take(room.settings.max_players) {
            room.seats.push(Seat {
                who,
                ready: false,
                slot: None,
                wins: 0,
            });
        }
        room.begin_countdown();
        room
    }

    pub fn players(&self) -> usize {
        self.seats.len()
    }

    /// Whether no person is left: bots don't keep a room open.
    pub fn is_empty(&self) -> bool {
        self.seats.iter().all(|s| is_bot(s.who.id))
    }

    /// The room's host: the person in it longest.
    fn host(&self) -> Option<&Seat> {
        self.seats.iter().find(|s| !is_bot(s.who.id))
    }

    fn hosts(&self, id: PlayerId) -> bool {
        self.host().is_some_and(|s| s.who.id == id)
    }

    pub fn contains(&self, id: PlayerId) -> bool {
        self.seat(id).is_some()
    }

    fn seat(&self, id: PlayerId) -> Option<&Seat> {
        self.seats.iter().find(|s| s.who.id == id)
    }

    fn seat_mut(&mut self, id: PlayerId) -> Option<&mut Seat> {
        self.seats.iter_mut().find(|s| s.who.id == id)
    }

    /// Whether a round is being played right now (including its countdown).
    pub fn in_play(&self) -> bool {
        matches!(self.phase, Phase::Countdown { .. } | Phase::Running)
    }

    /// Whether `id` has a part in the round being played.
    pub fn plays(&self, id: PlayerId) -> bool {
        self.in_play() && self.seat(id).is_some_and(|s| s.slot.is_some())
    }

    /// `who` takes over the seat `old` has in the round being played, as
    /// when a player who lost their connection comes back on a new one.
    /// Only in games whose rounds know players by slot. Says whether there
    /// was such a seat.
    pub fn rejoin(&mut self, old: PlayerId, who: Entrant) -> bool {
        if !G::REJOIN || !self.plays(old) || (old != who.id && self.contains(who.id)) {
            return false;
        }
        let Some(seat) = self.seat_mut(old) else {
            return false;
        };
        seat.who = who.clone();
        if let Some(slot) = seat.slot {
            self.players[slot as usize] = who;
        }
        self.changed = true;
        true
    }

    /// This round, from its countdown until the next round's lobby.
    pub fn round(&self) -> Option<&G::Round> {
        self.round.as_ref()
    }

    /// This round, for tests that set it up by hand.
    pub fn round_mut(&mut self) -> Option<&mut G::Round> {
        self.round.as_mut()
    }

    /// Takes a seat. Players who join during a round watch it and play the
    /// next one.
    pub fn join(&mut self, who: Entrant) -> Result<(), JoinError> {
        if self.contains(who.id) {
            return Ok(());
        }
        if self.settings.kind == RoomKind::Match {
            return Err(JoinError::Closed);
        }
        if self.seats.len() >= self.settings.max_players {
            // A person takes a bot's seat, between rounds.
            let bot = self.seats.iter().rposition(|s| is_bot(s.who.id));
            match bot {
                Some(i) if !self.in_play() => {
                    self.seats.remove(i);
                }
                _ => return Err(JoinError::Full),
            }
        }
        self.seats.push(Seat {
            who,
            ready: false,
            slot: None,
            wins: 0,
        });
        self.arm_start_timer();
        self.changed = true;
        Ok(())
    }

    /// Gives up a seat. A player still in play is out on the spot, which
    /// can decide the round.
    pub fn leave(&mut self, id: PlayerId) -> Option<RoundEnd> {
        let i = self.seats.iter().position(|s| s.who.id == id)?;
        let seat = self.seats.remove(i);
        self.changed = true;
        let in_play = self.in_play();
        let end = match (seat.slot, self.round.as_mut()) {
            (Some(slot), Some(round)) if in_play => round.leave(slot, self.clock),
            _ => None,
        };
        if let Phase::Waiting { start_at } = &mut self.phase
            && self.seats.len() < MIN_PLAYERS
        {
            *start_at = None;
        }
        end.map(|end| self.finish(end))
    }

    /// Marks a player ready, or not ready again. Once everyone in a private
    /// room is ready, the countdown starts.
    pub fn toggle_ready(&mut self, id: PlayerId) {
        if !matches!(self.phase, Phase::Waiting { .. }) {
            return;
        }
        let Some(seat) = self.seat_mut(id) else {
            return;
        };
        seat.ready = !seat.ready;
        self.changed = true;
        if self.seats.len() >= MIN_PLAYERS && self.seats.iter().all(|s| s.ready || is_bot(s.who.id))
        {
            self.begin_countdown();
        }
    }

    /// The host adds a bot, in a private room's lobby with a seat to
    /// spare. Says whether one joined.
    pub fn add_bot(&mut self, id: PlayerId) -> bool {
        if !self.lobby_by(id) || self.seats.len() >= self.settings.max_players {
            return false;
        }
        // The first name no bot here has.
        let Some(n) = (0..G::BOT_NAMES.len()).find(|&n| !self.contains(G::bot(n).id)) else {
            return false;
        };
        self.seats.push(Seat {
            who: G::bot(n),
            ready: false,
            slot: None,
            wins: 0,
        });
        self.lobby_changed();
        true
    }

    /// The host takes out the bot that joined last. Says whether there was
    /// one.
    pub fn remove_bot(&mut self, id: PlayerId) -> bool {
        if !self.lobby_by(id) {
            return false;
        }
        let Some(i) = self.seats.iter().rposition(|s| is_bot(s.who.id)) else {
            return false;
        };
        self.seats.remove(i);
        self.lobby_changed();
        true
    }

    /// Whether `id` hosts this private room, and it's between rounds.
    fn lobby_by(&self, id: PlayerId) -> bool {
        self.settings.kind == RoomKind::Private
            && self.hosts(id)
            && matches!(self.phase, Phase::Waiting { .. })
    }

    /// Who plays, or by what rules, just changed: nobody is ready any more,
    /// and the start timer starts again, so nobody starts a round they
    /// didn't see.
    fn lobby_changed(&mut self) {
        let enough = self.seats.len() >= MIN_PLAYERS;
        let restart = self.settings.start_after.map(|after| self.clock + after);
        if let Phase::Waiting { start_at } = &mut self.phase {
            *start_at = restart.filter(|_| enough);
        }
        for seat in &mut self.seats {
            seat.ready = false;
        }
        self.changed = true;
    }

    /// The room's host (whoever has been in it longest) changes the
    /// settings of its next rounds: in a private room's lobby only, to
    /// settings the game allows, with a seat for everyone already in.
    /// Everyone is ready no longer and the start timer starts again, so
    /// nobody plays by settings they didn't see. Says whether they changed.
    pub fn set_settings(&mut self, id: PlayerId, settings: G::Settings) -> bool {
        // The cheap checks first: a flood of the same change costs nothing.
        if !self.lobby_by(id)
            || G::for_rooms(settings.clone()) == self.settings.game
            || !G::allowed(&settings)
        {
            return false;
        }
        let Some(next) = RoomSettings::<G>::new(settings, self.settings.kind) else {
            return false;
        };
        if next.max_players < self.seats.len() {
            return false;
        }
        self.settings.game = next.game;
        self.settings.max_players = next.max_players;
        self.lobby_changed();
        true
    }

    /// A move by this player, during the countdown or play.
    pub fn play(
        &mut self,
        id: PlayerId,
        input: G::Input,
        rtt: Option<Duration>,
    ) -> Option<RoundEnd> {
        if !self.in_play() {
            return None;
        }
        let slot = self.seat(id)?.slot?;
        let end = self.round.as_mut()?.input(slot, input, self.clock, rtt)?;
        Some(self.finish(end))
    }

    /// Moves the room's clock forward by `dt`.
    pub fn advance(&mut self, dt: Duration) -> Option<RoundEnd> {
        self.clock += dt;
        let mut end = None;
        match self.phase.clone() {
            Phase::Waiting { start_at: Some(at) } if self.clock >= at => self.begin_countdown(),
            Phase::Countdown { go_at } if self.clock >= go_at => {
                self.phase = Phase::Running;
                self.changed = true;
                // Someone may have left during the countdown.
                end = self.round.as_mut().and_then(|r| r.go(self.clock));
            }
            Phase::Running => end = self.round.as_mut().and_then(|r| r.advance(self.clock, dt)),
            Phase::Results { next_at: Some(at) } if self.clock >= at => self.back_to_waiting(),
            _ => {}
        }
        let end = end.map(|end| self.finish(end));
        let shown = self.shown_secs();
        if shown != self.shown_secs {
            self.shown_secs = shown;
            self.changed = true;
        }
        end
    }

    /// Time until something on screen changes: whatever the round says
    /// while it plays, otherwise the next second of a countdown. A room can
    /// sleep this long.
    pub fn until_next(&self) -> Duration {
        const SOON: Duration = Duration::from_millis(250);
        let until = |at: Duration| at.saturating_sub(self.clock);
        match &self.phase {
            Phase::Running => self
                .round
                .as_ref()
                .map_or(SOON, |r| r.until_next(self.clock)),
            Phase::Waiting { start_at: Some(at) }
            | Phase::Countdown { go_at: at }
            | Phase::Results { next_at: Some(at) } => until(*at).min(SOON),
            _ => Duration::from_secs(1),
        }
    }

    /// Whether anything players see has changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        let round = self.round.as_mut().is_some_and(|r| r.take_changed());
        std::mem::take(&mut self.changed) | round
    }

    /// Whole seconds left on whatever is counting down.
    fn shown_secs(&self) -> Option<u64> {
        let left = |at: Duration| at.saturating_sub(self.clock);
        let left = match &self.phase {
            Phase::Waiting { start_at: Some(at) }
            | Phase::Countdown { go_at: at }
            | Phase::Results { next_at: Some(at) } => Some(left(*at)),
            Phase::Running => self.round.as_ref().and_then(|r| r.time_left(self.clock)),
            _ => None,
        };
        left.map(|left| left.as_secs_f64().ceil() as u64)
    }

    fn arm_start_timer(&mut self) {
        if let Phase::Waiting { start_at } = &mut self.phase
            && start_at.is_none()
            && self.seats.len() >= MIN_PLAYERS
        {
            *start_at = self.settings.start_after.map(|after| self.clock + after);
        }
    }

    fn begin_countdown(&mut self) {
        let players: Vec<Entrant> = self.seats.iter().map(|s| s.who.clone()).collect();
        let Some(round) = G::Round::start(&self.settings.game, &players, &mut self.rng) else {
            self.phase = Phase::Waiting { start_at: None };
            return;
        };
        for (i, seat) in self.seats.iter_mut().enumerate() {
            seat.slot = Some(i as u8);
            seat.ready = false;
        }
        self.started = players.iter().map(|p| p.id).collect();
        self.players = players;
        self.round = Some(round);
        self.placements.clear();
        self.phase = Phase::Countdown {
            go_at: self.clock + self.settings.countdown,
        };
        self.changed = true;
    }

    /// Shows how the round ended, and counts the win.
    fn finish(&mut self, mut end: RoundEnd) -> RoundEnd {
        // Players who came back as someone new are named as they are now.
        if let RoundEnd::Ranked(placements) = &mut end {
            for p in placements {
                if let Some(slot) = self.started.iter().position(|&id| id == p.who.id) {
                    p.who = self.players[slot].clone();
                }
            }
        }
        self.placements = end.placements().to_vec();
        if let Some(id) = winner(&self.placements).map(|w| w.id)
            && let Some(seat) = self.seat_mut(id)
        {
            seat.wins += 1;
        }
        let next_at = match self.settings.kind {
            RoomKind::Private => Some(self.clock + self.settings.results_for),
            RoomKind::Match => None,
        };
        self.phase = Phase::Results { next_at };
        self.changed = true;
        end
    }

    fn back_to_waiting(&mut self) {
        self.round = None;
        self.players.clear();
        self.started.clear();
        self.placements.clear();
        for seat in &mut self.seats {
            seat.slot = None;
            seat.ready = false;
        }
        self.phase = Phase::Waiting { start_at: None };
        self.arm_start_timer();
        self.changed = true;
    }

    pub fn view(&self) -> RoomView<G> {
        let left = |at: Duration| at.saturating_sub(self.clock);
        let phase = match &self.phase {
            Phase::Waiting { start_at } => PhaseView::Waiting {
                starts_in: start_at.map(left),
            },
            Phase::Countdown { go_at } => PhaseView::Countdown { left: left(*go_at) },
            Phase::Running => PhaseView::Running {
                left: self.round.as_ref().and_then(|r| r.time_left(self.clock)),
            },
            Phase::Results { next_at } => PhaseView::Results {
                next_in: next_at.map(left),
            },
        };
        let mut seats: Vec<SeatView> = self
            .seats
            .iter()
            .map(|s| SeatView {
                who: s.who.clone(),
                ready: s.ready || is_bot(s.who.id),
                slot: s.slot,
                wins: s.wins,
                left: false,
                ping: None,
                away: None,
            })
            .collect();
        // Players who left mid-round stay listed until it's over.
        for (i, who) in self.players.iter().enumerate() {
            if !self.contains(who.id) {
                seats.push(SeatView {
                    who: who.clone(),
                    ready: false,
                    slot: Some(i as u8),
                    wins: 0,
                    left: true,
                    ping: None,
                    away: None,
                });
            }
        }
        RoomView {
            shell: ShellView {
                kind: self.settings.kind,
                code: None,
                max_players: self.settings.max_players,
                phase,
                seats,
                placements: self.placements.clone(),
                ratings: Vec::new(),
                watchers: 0,
            },
            settings: self.settings.game.clone(),
            game: self
                .round
                .as_ref()
                .map_or_else(G::View::default, |r| r.view(self.clock)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GameId, MIN_PLAYERS};

    /// A race to press a key `target` times. A target of 0 seats nobody,
    /// and 99 never starts.
    struct Presses;

    #[derive(Clone, Debug, Default, PartialEq)]
    struct Scores {
        presses: Vec<u8>,
        went: bool,
    }

    struct PressRound {
        target: u8,
        players: Vec<Entrant>,
        presses: Vec<u8>,
        out: Vec<bool>,
        went: bool,
        changed: bool,
    }

    impl Game for Presses {
        const ID: GameId = GameId::Snake;
        const BOT_NAMES: &'static [&'static str] = &["Ann", "Bob"];
        // Its rounds know players by slot, and name them in the results as
        // they began.
        const REJOIN: bool = true;
        type Settings = u8;
        type Input = ();
        type View = Scores;
        type Round = PressRound;

        fn allowed(target: &u8) -> bool {
            *target <= 100
        }

        fn seats(target: &u8) -> Option<usize> {
            (*target > 0).then_some(4)
        }
    }

    impl PressRound {
        /// Over once one player is left in.
        fn decided(&self) -> Option<RoundEnd> {
            let mut left = (0..self.players.len()).filter(|&i| !self.out[i]);
            match (left.next(), left.next()) {
                (Some(i), None) => Some(self.ranked(i)),
                _ => None,
            }
        }

        fn ranked(&self, first: usize) -> RoundEnd {
            let placements = (0..self.players.len())
                .map(|i| Placement {
                    who: self.players[i].clone(),
                    place: if i == first { 1 } else { 2 },
                })
                .collect();
            RoundEnd::Ranked(placements)
        }
    }

    impl Round<Presses> for PressRound {
        fn start(target: &u8, players: &[Entrant], _: &mut RoomRng) -> Option<Self> {
            (*target != 99).then(|| Self {
                target: *target,
                players: players.to_vec(),
                presses: vec![0; players.len()],
                out: vec![false; players.len()],
                went: false,
                changed: false,
            })
        }

        fn go(&mut self, _: Duration) -> Option<RoundEnd> {
            self.went = true;
            self.decided()
        }

        fn advance(&mut self, _: Duration, _: Duration) -> Option<RoundEnd> {
            None
        }

        fn input(&mut self, slot: u8, _: (), _: Duration, _: Option<Duration>) -> Option<RoundEnd> {
            if !self.went {
                return None;
            }
            let i = slot as usize;
            self.presses[i] += 1;
            self.changed = true;
            (self.presses[i] >= self.target).then(|| self.ranked(i))
        }

        fn leave(&mut self, slot: u8, _: Duration) -> Option<RoundEnd> {
            self.out[slot as usize] = true;
            if self.went { self.decided() } else { None }
        }

        fn until_next(&self, _: Duration) -> Duration {
            Duration::from_secs(1)
        }

        fn view(&self, _: Duration) -> Scores {
            Scores {
                presses: self.presses.clone(),
                went: self.went,
            }
        }

        fn take_changed(&mut self) -> bool {
            std::mem::take(&mut self.changed)
        }
    }

    const S: Duration = Duration::from_secs(1);

    fn who(id: PlayerId) -> Entrant {
        Entrant {
            id,
            name: format!("p{id}"),
        }
    }

    fn private(target: u8) -> Room<Presses> {
        Room::new(RoomSettings::new(target, RoomKind::Private).unwrap(), 1)
    }

    fn matched(players: u64) -> Room<Presses> {
        let settings = RoomSettings::new(2, RoomKind::Match).unwrap();
        Room::new_match(settings, 1, (1..=players).map(who).collect())
    }

    fn phase(room: &Room<Presses>) -> PhaseView {
        room.view().shell.phase
    }

    #[test]
    fn a_private_room_starts_after_the_timer_or_when_everyone_is_ready() {
        let mut room = private(3);
        room.join(who(1)).unwrap();
        assert_eq!(phase(&room), PhaseView::Waiting { starts_in: None });
        room.join(who(2)).unwrap();
        assert_eq!(
            phase(&room),
            PhaseView::Waiting {
                starts_in: Some(15 * S)
            }
        );
        room.advance(14 * S);
        assert!(matches!(phase(&room), PhaseView::Waiting { .. }));
        room.advance(S);
        assert_eq!(phase(&room), PhaseView::Countdown { left: 3 * S });

        let mut ready = private(3);
        ready.join(who(1)).unwrap();
        ready.join(who(2)).unwrap();
        ready.toggle_ready(1);
        ready.toggle_ready(2);
        let view = ready.view();
        assert!(matches!(view.shell.phase, PhaseView::Countdown { .. }));
        let slots: Vec<Option<u8>> = view.shell.seats.iter().map(|s| s.slot).collect();
        assert_eq!(slots, [Some(0), Some(1)]);
        assert_eq!(
            view.game.presses,
            [0, 0],
            "the round shows from its countdown"
        );
    }

    #[test]
    fn a_player_back_as_someone_new_takes_their_seat() {
        let mut room = matched(2);
        room.advance(3 * S);
        assert!(!room.rejoin(5, who(9)), "nobody's seat");
        assert!(!room.rejoin(2, who(1)), "someone already seated");
        assert!(room.rejoin(1, who(11)));
        assert!(!room.contains(1) && room.plays(11));
        let seat = room.view().shell.seats[0].clone();
        assert_eq!((seat.who, seat.slot, seat.left), (who(11), Some(0), false));
        room.play(11, (), None);
        let end = room.play(11, (), None).expect("their presses count");
        assert_eq!(end.placements()[0].who, who(11), "named as they are now");
        let view = room.view();
        assert_eq!(view.shell.winner(), Some(&who(11)));
        assert_eq!(view.shell.seats[0].wins, 1);
        assert!(!room.rejoin(11, who(12)), "the round is over");
    }

    #[test]
    fn a_room_without_a_timer_waits_for_everyone() {
        let mut settings = RoomSettings::<Presses>::new(3, RoomKind::Private).unwrap();
        settings.start_after = None;
        let mut room = Room::new(settings, 1);
        room.join(who(1)).unwrap();
        room.join(who(2)).unwrap();
        room.advance(60 * S);
        assert_eq!(phase(&room), PhaseView::Waiting { starts_in: None });
        room.toggle_ready(1);
        room.toggle_ready(2);
        assert!(matches!(phase(&room), PhaseView::Countdown { .. }));
    }

    #[test]
    fn moves_count_from_the_countdown_and_the_winner_wins() {
        let mut room = matched(2);
        assert_eq!(
            room.play(1, (), None),
            None,
            "the round decides what counts"
        );
        room.advance(3 * S);
        assert_eq!(phase(&room), PhaseView::Running { left: None });
        assert_eq!(room.play(1, (), None), None);
        assert_eq!(room.play(9, (), None), None, "not in the room");
        let end = room.play(1, (), None).unwrap();
        assert_eq!(end.placements()[0].who, who(1));
        let view = room.view();
        assert_eq!(view.shell.phase, PhaseView::Results { next_in: None });
        assert_eq!(view.shell.winner(), Some(&who(1)));
        assert_eq!(view.shell.seats[0].wins, 1);
        assert_eq!(
            view.game.presses,
            [2, 0],
            "the round shows with its results"
        );
        assert_eq!(room.play(2, (), None), None, "over");
    }

    #[test]
    fn the_host_changes_the_settings_in_the_lobby() {
        let mut room = private(3);
        room.join(who(1)).unwrap();
        room.join(who(2)).unwrap();
        room.toggle_ready(2);
        room.advance(10 * S);
        assert_eq!(room.view().shell.host().map(|s| s.who.id), Some(1));
        assert!(!room.set_settings(2, 5), "only the host");
        assert!(!room.set_settings(1, 101), "not allowed");
        assert!(!room.set_settings(1, 0), "seats nobody");
        assert!(!room.set_settings(1, 3), "no change");
        assert!(room.set_settings(1, 5));
        let view = room.view();
        assert_eq!(view.settings, 5);
        assert!(view.shell.seats.iter().all(|s| !s.ready), "ready again");
        assert_eq!(
            view.shell.phase,
            PhaseView::Waiting {
                starts_in: Some(15 * S)
            },
            "the start timer starts again"
        );
        room.toggle_ready(1);
        room.toggle_ready(2);
        assert!(!room.set_settings(1, 7), "not once the round is on");
        assert!(!matched(2).set_settings(1, 7), "matches keep theirs");
    }

    #[test]
    fn the_host_adds_and_removes_bots_in_the_lobby() {
        let mut room = private(3);
        room.join(who(1)).unwrap();
        assert!(room.add_bot(1));
        assert!(!room.add_bot(2), "only the host");
        room.join(who(2)).unwrap();
        assert!(room.add_bot(1));
        assert!(!room.add_bot(1), "no names left");
        let names: Vec<String> = room
            .view()
            .shell
            .seats
            .into_iter()
            .map(|s| s.who.name)
            .collect();
        assert_eq!(names, ["p1", "Ann", "p2", "Bob"]);
        assert!(
            room.view()
                .shell
                .seats
                .iter()
                .filter(|s| is_bot(s.who.id))
                .all(|s| s.ready)
        );
        assert!(room.remove_bot(1));
        assert!(room.add_bot(1), "the free name again");

        // Full: a person takes a bot's seat between rounds.
        room.join(who(3)).unwrap();
        assert_eq!(room.players(), 4);
        assert!(room.contains(3));
        room.join(who(4)).unwrap();
        assert_eq!(room.join(who(5)), Err(crate::JoinError::Full));
        // When the people go, so does the room.
        for id in 1..=4 {
            room.leave(id);
        }
        assert!(room.is_empty());
        assert!(!matched(2).add_bot(1), "matches have who they have");
    }

    #[test]
    fn leaving_can_decide_the_round_and_leavers_stay_listed() {
        let mut room = matched(3);
        room.advance(3 * S);
        assert_eq!(room.leave(1), None, "two still playing");
        let gone = room
            .view()
            .shell
            .seats
            .into_iter()
            .find(|s| s.left)
            .unwrap();
        assert_eq!((gone.who.id, gone.slot), (1, Some(0)), "listed this round");
        let end = room.leave(2).unwrap();
        assert_eq!(room.view().shell.winner(), Some(&who(3)));
        assert_eq!(end.placements().len(), 3, "players who left are ranked");
        assert_eq!(room.leave(2), None, "already gone");
    }

    #[test]
    fn leaving_during_the_countdown_ends_the_round_as_it_starts() {
        let mut room = matched(2);
        assert_eq!(room.leave(1), None);
        let end = room.advance(3 * S);
        assert!(end.is_some());
        assert_eq!(room.view().shell.winner(), Some(&who(2)));
    }

    #[test]
    fn private_rooms_go_round_after_round() {
        let mut room = private(1);
        room.join(who(1)).unwrap();
        room.join(who(2)).unwrap();
        room.toggle_ready(1);
        room.toggle_ready(2);
        room.advance(3 * S);
        room.play(2, (), None).unwrap();
        assert_eq!(
            phase(&room),
            PhaseView::Results {
                next_in: Some(8 * S)
            }
        );
        // Someone arriving now watches, then plays the next round.
        room.join(who(3)).unwrap();
        assert_eq!(room.view().shell.seat(3).unwrap().slot, None);
        room.advance(8 * S);
        let view = room.view();
        assert!(matches!(
            view.shell.phase,
            PhaseView::Waiting { starts_in: Some(_) }
        ));
        assert_eq!(view.game, Scores::default(), "no round between rounds");
        assert!(view.shell.placements.is_empty());
        for id in 1..=3 {
            room.toggle_ready(id);
        }
        assert_eq!(room.view().shell.seat(3).unwrap().slot, Some(2));
        assert_eq!(room.view().shell.seat(2).unwrap().wins, 1, "wins are kept");
    }

    #[test]
    fn a_round_that_cant_start_leaves_the_room_waiting() {
        let mut room = private(99);
        room.join(who(1)).unwrap();
        room.join(who(2)).unwrap();
        room.advance(15 * S);
        assert_eq!(phase(&room), PhaseView::Waiting { starts_in: None });
        assert!(room.round().is_none());
        assert!(room.players() >= MIN_PLAYERS);
    }

    #[test]
    fn matches_take_no_one_else() {
        let mut room = matched(2);
        assert_eq!(room.join(who(3)), Err(crate::JoinError::Closed));
        assert_eq!(room.join(who(1)), Ok(()), "already in");
    }

    #[test]
    fn changes_are_reported_once() {
        let mut room = private(3);
        assert!(room.take_changed());
        assert!(!room.take_changed());
        room.join(who(1)).unwrap();
        assert!(room.take_changed());
        room.advance(Duration::from_millis(10));
        assert!(!room.take_changed(), "nothing on screen moved");

        let mut room = matched(2);
        room.advance(3 * S);
        room.take_changed();
        room.play(1, (), None);
        assert!(room.take_changed(), "the round changed");
    }
}
