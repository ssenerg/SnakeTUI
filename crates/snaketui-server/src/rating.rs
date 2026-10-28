//! Skill ratings for quick play: Weng-Lin, which rates free-for-all games
//! of any size, for Snake; and a [`Skill`] in either system, Glicko-2
//! being chess's.

use serde::{Deserialize, Serialize};
use skillratings::MultiTeamOutcome;
use skillratings::weng_lin::{WengLinConfig, WengLinRating, weng_lin_multi_team};

use crate::games::RatingSystem;

pub use game_api::RatingKey;

/// Snake's quick play, every snake for itself.
pub const SNAKE_QUICK: RatingKey = snake_core::round::QUICK_RATING;

/// A player's estimated skill, `mu`, and how unsure that estimate is,
/// `sigma`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rating {
    pub mu: f64,
    pub sigma: f64,
}

impl Default for Rating {
    fn default() -> Self {
        let start = WengLinRating::new();
        Self {
            mu: start.rating,
            sigma: start.uncertainty,
        }
    }
}

impl Rating {
    /// The number players see. A new player shows 1000.
    pub fn shown(self) -> u32 {
        (self.mu * 40.0).round().max(0.0) as u32
    }
}

/// Everyone's new rating after a round, from their rating and place (1 is
/// first; equal places drew), in the same order.
pub fn after_round(players: &[(Rating, usize)]) -> Vec<Rating> {
    let ratings: Vec<[WengLinRating; 1]> = players
        .iter()
        .map(|(r, _)| {
            [WengLinRating {
                rating: r.mu,
                uncertainty: r.sigma,
            }]
        })
        .collect();
    let teams: Vec<(&[WengLinRating], MultiTeamOutcome)> = ratings
        .iter()
        .zip(players)
        .map(|(r, (_, place))| (&r[..], MultiTeamOutcome::new(*place)))
        .collect();
    weng_lin_multi_team(&teams, &WengLinConfig::new())
        .into_iter()
        .map(|team| Rating {
            mu: team[0].rating,
            sigma: team[0].uncertainty,
        })
        .collect()
}

/// A player's rating in one category, in the system its game rates by.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Skill {
    /// Snake's: see [`Rating`].
    WengLin(Rating),
    /// Chess's: see [`chess_core::rating`].
    Glicko2(chess_core::rating::Rating),
}

impl Skill {
    /// Where a new player starts in `system`.
    pub fn new(system: RatingSystem) -> Skill {
        match system {
            RatingSystem::WengLin => Skill::WengLin(Rating::default()),
            RatingSystem::Glicko2 => Skill::Glicko2(chess_core::rating::Rating::default()),
        }
    }

    /// The number players see.
    pub fn shown(self) -> u32 {
        match self {
            Skill::WengLin(r) => r.shown(),
            Skill::Glicko2(r) => r.shown(),
        }
    }

    /// Whether it's still too unsure to count for much, after `idle_days`
    /// without a rated game: shown with a question mark.
    pub fn is_provisional(self, idle_days: u32) -> bool {
        match self {
            Skill::WengLin(_) => false,
            Skill::Glicko2(r) => r.decayed(idle_days).is_provisional(),
        }
    }
}

impl From<Rating> for Skill {
    fn from(rating: Rating) -> Skill {
        Skill::WengLin(rating)
    }
}

#[cfg(test)]
impl Skill {
    /// Weng-Lin's mu, for tests of Snake's ratings.
    pub fn mu(self) -> f64 {
        match self {
            Skill::WengLin(r) => r.mu,
            Skill::Glicko2(_) => panic!("not a Weng-Lin rating"),
        }
    }

    pub fn sigma(self) -> f64 {
        match self {
            Skill::WengLin(r) => r.sigma,
            Skill::Glicko2(_) => panic!("not a Weng-Lin rating"),
        }
    }
}

/// Whole days from `from` to `to`, as Unix times, for ratings that grow
/// unsure with time; none if `to` comes first.
pub fn days_between(from: i64, to: i64) -> u32 {
    u32::try_from((to - from).max(0) / 86_400).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_players_show_1000() {
        assert_eq!(Rating::default().shown(), 1000);
    }

    #[test]
    fn chess_ratings_start_unsure_and_grow_unsure_again() {
        let new = Skill::new(RatingSystem::Glicko2);
        assert_eq!(new.shown(), 1500);
        assert!(new.is_provisional(0));
        let settled = Skill::Glicko2(chess_core::rating::Rating {
            rating: 1612.0,
            deviation: 60.0,
            volatility: 0.06,
        });
        assert!(!settled.is_provisional(0));
        assert!(settled.is_provisional(3650), "ten years away");
        assert!(!Skill::new(RatingSystem::WengLin).is_provisional(3650));
        assert_eq!(days_between(0, 86_400 * 3 - 1), 2);
        assert_eq!(days_between(10, 0), 0);
    }

    #[test]
    fn winning_raises_a_rating_and_losing_lowers_it() {
        let start = Rating::default();
        let after = after_round(&[(start, 1), (start, 2)]);
        assert!(after[0].mu > start.mu);
        assert!(after[1].mu < start.mu);
        assert!(after[0].sigma < start.sigma, "more certain after a game");
    }

    #[test]
    fn places_order_the_changes_and_draws_change_little() {
        let start = Rating::default();
        let after = after_round(&[(start, 3), (start, 1), (start, 2)]);
        assert!(after[1].mu > after[2].mu && after[2].mu > after[0].mu);

        let drawn = after_round(&[(start, 1), (start, 1)]);
        assert!((drawn[0].mu - start.mu).abs() < 1e-9);
    }

    #[test]
    fn beating_a_stronger_player_counts_for_more() {
        let weak = Rating {
            mu: 20.0,
            sigma: 3.0,
        };
        let strong = Rating {
            mu: 30.0,
            sigma: 3.0,
        };
        let upset = after_round(&[(weak, 1), (strong, 2)]);
        let expected = after_round(&[(strong, 1), (weak, 2)]);
        assert!(upset[0].mu - weak.mu > expected[0].mu - strong.mu);
    }
}
