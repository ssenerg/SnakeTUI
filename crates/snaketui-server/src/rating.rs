//! Skill ratings for quick play: Weng-Lin, which rates free-for-all games
//! of any size.

use skillratings::MultiTeamOutcome;
use skillratings::weng_lin::{WengLinConfig, WengLinRating, weng_lin_multi_team};

/// A player's estimated skill, `mu`, and how unsure that estimate is,
/// `sigma`.
#[derive(Clone, Copy, Debug, PartialEq)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_players_show_1000() {
        assert_eq!(Rating::default().shown(), 1000);
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
