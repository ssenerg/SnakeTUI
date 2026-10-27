//! Chess ratings: Glicko-2, kept apart for each
//! [`crate::control::Category`]. A rating's deviation, how unsure it is,
//! grows for every day its player doesn't play there, and while it's large
//! the rating is provisional.

use skillratings::Outcomes;
use skillratings::glicko2::{Glicko2Config, Glicko2Rating, glicko2};

use crate::piece::Color;

/// Glicko-2's tau: how quickly a player's volatility may change.
pub const TAU: f64 = 0.5;
/// A rating is provisional while its deviation is above this.
pub const PROVISIONAL_DEVIATION: f64 = 110.0;
/// Where a new player's deviation starts, and the most idleness grows it to.
const MAX_DEVIATION: f64 = 350.0;
/// Rating points to one unit of Glicko-2's own scale.
const SCALE: f64 = 173.7178;

#[derive(Clone, Copy, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rating {
    pub rating: f64,
    /// How far off `rating` may be: 350 for a new player.
    pub deviation: f64,
    /// How erratic the player's results are.
    pub volatility: f64,
}

/// A new player: 1500, deviation 350, volatility 0.06.
impl Default for Rating {
    fn default() -> Self {
        Glicko2Rating::new().into()
    }
}

impl From<Glicko2Rating> for Rating {
    fn from(r: Glicko2Rating) -> Self {
        Rating {
            rating: r.rating,
            deviation: r.deviation,
            volatility: r.volatility,
        }
    }
}

impl From<Rating> for Glicko2Rating {
    fn from(r: Rating) -> Self {
        Glicko2Rating {
            rating: r.rating,
            deviation: r.deviation,
            volatility: r.volatility,
        }
    }
}

impl Rating {
    /// The number players see.
    pub fn shown(self) -> u32 {
        self.rating.round().max(0.0) as u32
    }

    /// Whether the rating is still too unsure to count for much: shown
    /// with a question mark, say.
    pub fn is_provisional(self) -> bool {
        self.deviation > PROVISIONAL_DEVIATION
    }

    /// The rating after `days` without a game, each day a rating period in
    /// which the deviation grows as [`skillratings::glicko2::decay_deviation`]
    /// grows it, up to a new player's. Worked out in one go, so a long
    /// absence costs no more than a short one.
    pub fn decayed(self, days: u32) -> Rating {
        if days == 0 {
            return self;
        }
        let phi = self.deviation / SCALE;
        let grown = (phi * phi + f64::from(days) * self.volatility * self.volatility).sqrt();
        Rating {
            deviation: (grown * SCALE).min(MAX_DEVIATION),
            ..self
        }
    }
}

/// A player going into a rated game: their rating in the game's category,
/// and how many whole days have passed since their last rated game there.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Entrant {
    pub rating: Rating,
    pub idle_days: u32,
}

/// White's and black's ratings after a game `winner` won, or that was
/// drawn if `None`. Each rating first decays for its player's idle days.
pub fn after_game(white: Entrant, black: Entrant, winner: Option<Color>) -> (Rating, Rating) {
    let white: Glicko2Rating = white.rating.decayed(white.idle_days).into();
    let black: Glicko2Rating = black.rating.decayed(black.idle_days).into();
    let outcome = match winner {
        Some(Color::White) => Outcomes::WIN,
        Some(Color::Black) => Outcomes::LOSS,
        None => Outcomes::DRAW,
    };
    let config = Glicko2Config {
        tau: TAU,
        ..Glicko2Config::new()
    };
    let (white, black) = glicko2(&white, &black, &outcome, &config);
    (white.into(), black.into())
}

#[cfg(test)]
mod tests {
    use skillratings::glicko2::decay_deviation;

    use super::*;

    fn fresh() -> Entrant {
        Entrant {
            rating: Rating::default(),
            idle_days: 0,
        }
    }

    fn settled(rating: f64, deviation: f64) -> Entrant {
        Entrant {
            rating: Rating {
                rating,
                deviation,
                volatility: 0.06,
            },
            idle_days: 0,
        }
    }

    #[test]
    fn new_players_start_at_1500_and_provisional() {
        let start = Rating::default();
        assert_eq!(start.shown(), 1500);
        assert_eq!(start.deviation, 350.0);
        assert_eq!(start.volatility, 0.06);
        assert!(start.is_provisional());
    }

    #[test]
    fn winning_raises_a_rating_and_losing_lowers_it() {
        let (white, black) = after_game(fresh(), fresh(), Some(Color::White));
        assert_eq!((white.shown(), black.shown()), (1662, 1338));
        assert!(white.deviation < 350.0 && black.deviation < 350.0);

        let (white, black) = after_game(fresh(), fresh(), Some(Color::Black));
        assert_eq!((white.shown(), black.shown()), (1338, 1662));
    }

    #[test]
    fn a_draw_between_equals_changes_only_the_certainty() {
        let (white, black) = after_game(fresh(), fresh(), None);
        assert!((white.rating - 1500.0).abs() < 1e-9);
        assert!((black.rating - 1500.0).abs() < 1e-9);
        assert!(white.deviation < 350.0);
    }

    #[test]
    fn beating_a_stronger_player_counts_for_more() {
        let weak = settled(1400.0, 80.0);
        let strong = settled(1800.0, 80.0);
        let (upset, _) = after_game(weak, strong, Some(Color::White));
        let (expected, _) = after_game(strong, weak, Some(Color::White));
        assert!(upset.rating - 1400.0 > expected.rating - 1800.0);
        assert!(upset.rating - 1400.0 > 0.0 && expected.rating - 1800.0 > 0.0);
    }

    #[test]
    fn idle_days_make_a_rating_less_sure() {
        let rating = settled(1700.0, 60.0).rating;
        assert_eq!(rating.decayed(0), rating);

        let mut daily = Glicko2Rating::from(rating);
        for _ in 0..30 {
            daily = decay_deviation(&daily);
        }
        let month = rating.decayed(30);
        assert!((month.deviation - daily.deviation).abs() < 1e-9);
        assert_eq!(month.rating, 1700.0);
        assert!(month.deviation > rating.deviation);

        assert_eq!(rating.decayed(100_000).deviation, 350.0);
        assert_eq!(rating.decayed(u32::MAX).deviation, 350.0);
    }

    #[test]
    fn an_idle_players_rating_moves_further() {
        let active = settled(1600.0, 60.0);
        let idle = Entrant {
            idle_days: 365,
            ..active
        };
        let (after_active, _) = after_game(active, fresh(), Some(Color::Black));
        let (after_idle, _) = after_game(idle, fresh(), Some(Color::Black));
        assert!(after_idle.rating < after_active.rating);
        assert!(after_active.rating < 1600.0);
    }

    #[test]
    fn provisional_while_the_deviation_is_above_110() {
        assert!(!settled(1500.0, 110.0).rating.is_provisional());
        assert!(settled(1500.0, 110.5).rating.is_provisional());
    }

    #[test]
    fn shown_ratings_are_rounded() {
        assert_eq!(settled(1499.5, 50.0).rating.shown(), 1500);
        assert_eq!(settled(1499.4, 50.0).rating.shown(), 1499);
        assert_eq!(settled(-20.0, 50.0).rating.shown(), 0);
    }

    #[test]
    fn many_games_settle_a_rating() {
        let mut player = fresh();
        for game in 0..40 {
            let opponent = settled(1500.0, 50.0);
            let winner = (game % 2 == 0).then_some(Color::White);
            player.rating = after_game(player, opponent, winner).0;
        }
        assert!(!player.rating.is_provisional(), "{:?}", player.rating);
        assert!(player.rating.rating > 1500.0, "won half and drew half");
    }
}
