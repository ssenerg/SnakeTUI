//! What a player's screen gets of a room: the same [`ShellView`] for every
//! game, with the game's settings and its own view of the round.

use std::any::Any;
use std::fmt;
use std::time::Duration;

use crate::{Entrant, Game, GameId, Placement, PlayerId, RoomKind, is_bot};

/// Everything a player's screen needs to draw a room.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(bound(
        serialize = "G::Settings: serde::Serialize, G::View: serde::Serialize",
        deserialize = "G::Settings: serde::Deserialize<'de>, G::View: serde::Deserialize<'de>"
    ))
)]
pub struct RoomView<G: Game> {
    pub shell: ShellView,
    /// What the room plays by.
    pub settings: G::Settings,
    /// The round, from its countdown until the next round's lobby;
    /// otherwise the game's default.
    pub game: G::View,
}

impl<G: Game> Clone for RoomView<G> {
    fn clone(&self) -> Self {
        Self {
            shell: self.shell.clone(),
            settings: self.settings.clone(),
            game: self.game.clone(),
        }
    }
}

impl<G: Game> fmt::Debug for RoomView<G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomView")
            .field("game", &G::ID)
            .field("shell", &self.shell)
            .field("settings", &self.settings)
            .field("view", &self.game)
            .finish()
    }
}

impl<G: Game> PartialEq for RoomView<G> {
    fn eq(&self, other: &Self) -> bool {
        self.shell == other.shell && self.settings == other.settings && self.game == other.game
    }
}

impl<G: Game> Eq for RoomView<G>
where
    G::Settings: Eq,
    G::View: Eq,
{
}

/// A room's view, of whichever game: what a player's screen and the
/// server hold without knowing the game. Its game reads the rest with
/// [`Self::of`].
pub trait AnyView: Any + Send + Sync + fmt::Debug {
    fn game(&self) -> GameId;
    fn shell(&self) -> &ShellView;
}

impl<G: Game> AnyView for RoomView<G> {
    fn game(&self) -> GameId {
        G::ID
    }

    fn shell(&self) -> &ShellView {
        &self.shell
    }
}

impl dyn AnyView {
    /// This view, if it's of a room of `G`.
    pub fn of<G: Game>(&self) -> Option<&RoomView<G>> {
        (self as &dyn Any).downcast_ref()
    }
}

/// The room around a round, the same for every game.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShellView {
    pub kind: RoomKind,
    /// Private rooms: the code others join with.
    pub code: Option<String>,
    pub max_players: usize,
    pub phase: PhaseView,
    /// In the order players joined, then anyone who left this round.
    pub seats: Vec<SeatView>,
    /// The last round's results, including players who have left.
    pub placements: Vec<Placement>,
    /// Rated rooms: how the last round moved players' ratings.
    pub ratings: Vec<RatingChange>,
    /// People watching who don't play. The server counts them.
    pub watchers: usize,
}

impl ShellView {
    /// This player's seat, unless they've left.
    pub fn seat(&self, id: PlayerId) -> Option<&SeatView> {
        self.seats.iter().find(|s| s.who.id == id && !s.left)
    }

    /// Private rooms: who sets the rules, the person in it longest.
    pub fn host(&self) -> Option<&SeatView> {
        self.seats
            .iter()
            .find(|s| !is_bot(s.who.id))
            .filter(|s| !s.left)
    }

    /// The last round's winner; `None` for a draw, or before any round.
    pub fn winner(&self) -> Option<&Entrant> {
        crate::winner(&self.placements)
    }
}

/// A player's shown rating before and after a rated round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RatingChange {
    pub id: PlayerId,
    pub before: u32,
    pub after: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PhaseView {
    Waiting {
        starts_in: Option<Duration>,
    },
    Countdown {
        left: Duration,
    },
    Running {
        /// Rounds with a time limit: what's left of it.
        left: Option<Duration>,
    },
    /// The round is over; its placements are the shell's.
    Results {
        /// Private rooms: the next round's lobby opens then.
        next_in: Option<Duration>,
    },
    /// The room is gone.
    Closed {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SeatView {
    pub who: Entrant,
    pub ready: bool,
    /// This player's place in this round's game; `None` while watching.
    pub slot: Option<u8>,
    /// Rounds won in this room.
    pub wins: u32,
    /// Left during this round; listed until it's over.
    pub left: bool,
    /// The player's round trip to the server, once measured. The server
    /// fills it in; a room knows nothing about networks.
    pub ping: Option<Duration>,
}
