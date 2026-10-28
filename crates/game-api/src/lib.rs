//! What every game on the server shares: who plays, the rooms they play
//! in, and the two traits a game implements to be played in one.
//!
//! A [`Game`] names its settings, its players' moves and what its rounds
//! show, and its [`Round`] plays one round. A [`Room`] does the rest the
//! same way for every game: seats, ready, the host, bots, the countdown,
//! results and wins. Nothing here knows about terminals or networks, and a
//! room only moves when told to, so tests drive its clock by hand.

#[cfg(feature = "serde")]
pub mod codec;
pub mod records;
pub mod room;
pub mod view;

use std::fmt::{self, Debug};
use std::time::Duration;

pub use records::Records;
pub use room::{Room, RoomRng, RoomSettings};
pub use view::{AnyView, PhaseView, RatingChange, RoomView, SeatView, ShellView};

/// Identifies a player to a room. The server hands them out; rooms never
/// interpret them.
pub type PlayerId = u64;

/// Fewer players than this can't play a round.
pub const MIN_PLAYERS: usize = 2;

/// Bots' ids have this bit set; the server never gives it to a person.
pub const BOT: PlayerId = 1 << 63;

/// Whether this player is a bot.
pub fn is_bot(id: PlayerId) -> bool {
    id & BOT != 0
}

/// Every game the server knows. Ids are only ever added: servers and
/// databases name games by them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum GameId {
    Snake,
    Chess,
}

impl GameId {
    /// Every game, in the order they were added.
    pub const ALL: &[GameId] = &[GameId::Snake, GameId::Chess];

    /// The name the database keeps it by.
    pub const fn as_str(self) -> &'static str {
        match self {
            GameId::Snake => "snake",
            GameId::Chess => "chess",
        }
    }

    /// The game kept by this name, if it's one of [`Self::ALL`].
    pub fn from_name(name: &str) -> Option<GameId> {
        Self::ALL.iter().copied().find(|g| g.as_str() == name)
    }
}

impl fmt::Display for GameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which of a player's ratings a game changes: each game keeps its own, in
/// categories such as chess's bullet and blitz.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RatingKey {
    pub game: GameId,
    pub category: &'static str,
}

/// A player as a room shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Entrant {
    pub id: PlayerId,
    /// Already safe to draw: the server checked it.
    pub name: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RoomKind {
    /// Formed by the matchmaker: one round with the players it picked.
    Match,
    /// Made by a player and joined by code: rounds until everyone leaves.
    Private,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum JoinError {
    Full,
    /// Matched rooms only take the players they were made for.
    Closed,
}

/// Where one player finished a round.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Placement {
    pub who: Entrant,
    /// 1 is first. Players who tie share a place, and the next place is
    /// skipped: 1, 2, 2, 4.
    pub place: usize,
}

/// How a round ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoundEnd {
    /// Everyone who played, best first, including players who left.
    Ranked(Vec<Placement>),
    /// Called off before it counted: nobody won or lost.
    Aborted,
}

impl RoundEnd {
    pub fn placements(&self) -> &[Placement] {
        match self {
            RoundEnd::Ranked(placements) => placements,
            RoundEnd::Aborted => &[],
        }
    }
}

/// Whoever finished first alone; `None` for a draw.
pub fn winner(placements: &[Placement]) -> Option<&Entrant> {
    let mut first = placements.iter().filter(|p| p.place == 1);
    match (first.next(), first.next()) {
        (Some(only), None) => Some(&only.who),
        _ => None,
    }
}

/// A game rooms can play.
pub trait Game: Sized + Send + Sync + 'static {
    const ID: GameId;
    /// Names for a room's bots, in the order they join. A game without
    /// bots has none.
    const BOT_NAMES: &'static [&'static str] = &[];
    /// Whether a player who loses their connection mid-round can come back
    /// to their seat, as someone new: see [`Room::rejoin`]. Its rounds must
    /// know players by slot, never by id.
    const REJOIN: bool = false;

    /// What a private room's host chooses, such as Snake's board.
    type Settings: Clone + Debug + PartialEq + Send + Sync + 'static;
    /// One move by one player.
    type Input: Debug + Send + 'static;
    /// What players see of a round, beside the room around it. The default
    /// is what shows between rounds.
    type View: Clone + Debug + Default + PartialEq + Send + Sync + 'static;
    type Round: Round<Self>;

    /// `settings` as rooms play them. Cheap: floods of changes call it.
    fn for_rooms(settings: Self::Settings) -> Self::Settings {
        settings
    }

    /// Whether a host may choose these settings: whether the settings
    /// screen could set them. Cheap, like [`Game::for_rooms`].
    fn allowed(settings: &Self::Settings) -> bool {
        let _ = settings;
        true
    }

    /// How many players a room with these settings seats; `None` if not
    /// even [`MIN_PLAYERS`] fit. It can be slow, so it's asked last.
    fn seats(settings: &Self::Settings) -> Option<usize>;

    /// A room's bot number `n`, from 0.
    fn bot(n: usize) -> Entrant {
        Entrant {
            id: BOT | n as PlayerId,
            name: Self::BOT_NAMES[n % Self::BOT_NAMES.len()].to_string(),
        }
    }
}

/// One round of a game, from its countdown to its results. Times are on
/// the room's clock.
pub trait Round<G: Game>: Send + Sized + 'static {
    /// A round for `players`, who have the slots they're listed in, as its
    /// countdown begins. `None` if one can't start, such as when no board
    /// fits them. The room's `rng` makes every random choice, so a room's
    /// seed replays the same rounds.
    fn start(settings: &G::Settings, players: &[Entrant], rng: &mut RoomRng) -> Option<Self>;

    /// The countdown is over at `at`: play starts.
    fn go(&mut self, at: Duration) -> Option<RoundEnd>;

    /// The clock moved on by `dt`, to `at`, while the round plays.
    fn advance(&mut self, at: Duration, dt: Duration) -> Option<RoundEnd>;

    /// A move by the player in `slot`, during the countdown or play. `rtt`
    /// is their latest round trip to the server, once measured.
    fn input(
        &mut self,
        slot: u8,
        input: G::Input,
        at: Duration,
        rtt: Option<Duration>,
    ) -> Option<RoundEnd>;

    /// The player in `slot` left during the countdown or play.
    fn leave(&mut self, slot: u8, at: Duration) -> Option<RoundEnd>;

    /// How long the room can sleep while the round plays before something
    /// on screen changes.
    fn until_next(&self, at: Duration) -> Duration;

    /// Time left in play, for rounds with a time limit.
    fn time_left(&self, at: Duration) -> Option<Duration> {
        let _ = at;
        None
    }

    fn view(&self, at: Duration) -> G::View;

    /// Whether anything players see has changed since the last call.
    fn take_changed(&mut self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_are_kept_by_name() {
        for &game in GameId::ALL {
            assert_eq!(GameId::from_name(game.as_str()), Some(game));
        }
        assert_eq!(GameId::from_name("checkers"), None);
        assert_eq!(GameId::from_name("Snake"), None, "names are lower case");
    }
}
