//! Who plays whom in quick play: players close in rating, from the same
//! region first, with both limits widening the longer they wait.
//!
//! Plain logic on its own clock, so tests drive it by hand;
//! [`crate::matchmaker::MatchmakerActor`] runs it once a second.

use std::time::Duration;

use snake_core::round::{MAX_PLAYERS, MIN_PLAYERS, PlayerId};

/// A match with fewer than a full room starts once its longest-waiting
/// player has waited this long.
pub const FILL_WAIT: Duration = Duration::from_secs(10);
/// Players from other regions are considered once both sides have waited
/// this long.
pub const REGION_WAIT: Duration = Duration::from_secs(20);
/// The rating window starts at ±`WINDOW_START`, grows by `WINDOW_GROWTH`
/// every `WINDOW_STEP`, and takes anyone after `WINDOW_OPEN`.
const WINDOW_START: f64 = 100.0;
const WINDOW_GROWTH: f64 = 50.0;
const WINDOW_STEP: Duration = Duration::from_secs(5);
const WINDOW_OPEN: Duration = Duration::from_secs(30);

/// How far apart in rating a player who has waited `waited` may be matched.
pub fn rating_window(waited: Duration) -> f64 {
    if waited >= WINDOW_OPEN {
        return f64::INFINITY;
    }
    let steps = (waited.as_secs() / WINDOW_STEP.as_secs()) as f64;
    WINDOW_START + WINDOW_GROWTH * steps
}

/// Whether a player who has waited `waited` may play people from other
/// regions.
pub fn regions_open(waited: Duration) -> bool {
    waited >= REGION_WAIT
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ticket {
    pub player: PlayerId,
    pub rating: f64,
    pub region: String,
    /// When they joined the queue, on the matcher's clock.
    pub since: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// The longest-waiting player first.
    pub players: Vec<PlayerId>,
    /// Where most of them are, which is where the room should go.
    pub region: String,
}

#[derive(Default)]
pub struct Matcher {
    /// Longest-waiting first.
    queue: Vec<Ticket>,
}

impl Matcher {
    /// Queues a player, replacing any ticket they already had.
    pub fn add(&mut self, ticket: Ticket) {
        self.remove(ticket.player);
        let at = self.queue.partition_point(|t| t.since <= ticket.since);
        self.queue.insert(at, ticket);
    }

    pub fn remove(&mut self, player: PlayerId) -> bool {
        let before = self.queue.len();
        self.queue.retain(|t| t.player != player);
        self.queue.len() != before
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Forms every match it can at time `now`, and takes those players out
    /// of the queue.
    ///
    /// Starting from the longest-waiting player, it gathers up to a full
    /// room of others within their rating window and allowed regions,
    /// taking the same region first and then the closest ratings. The match
    /// starts when the room is full, or with at least two players once that
    /// longest-waiting player has waited [`FILL_WAIT`].
    pub fn run(&mut self, now: Duration) -> Vec<Group> {
        let waited = |t: &Ticket| now.saturating_sub(t.since);
        let mut taken = vec![false; self.queue.len()];
        let mut groups = Vec::new();
        for a in 0..self.queue.len() {
            if taken[a] {
                continue;
            }
            let anchor = &self.queue[a];
            let window = rating_window(waited(anchor));
            let open = regions_open(waited(anchor));
            let mut picks: Vec<usize> = (0..self.queue.len())
                .filter(|&b| b != a && !taken[b])
                .filter(|&b| {
                    let other = &self.queue[b];
                    let region_ok =
                        other.region == anchor.region || (open && regions_open(waited(other)));
                    region_ok && (other.rating - anchor.rating).abs() <= window
                })
                .collect();
            picks.sort_by(|&x, &y| {
                let key = |i: usize| {
                    let t = &self.queue[i];
                    (t.region != anchor.region, (t.rating - anchor.rating).abs())
                };
                let (kx, ky) = (key(x), key(y));
                kx.0.cmp(&ky.0).then(kx.1.total_cmp(&ky.1))
            });
            picks.truncate(MAX_PLAYERS - 1);
            let size = picks.len() + 1;
            if size < MAX_PLAYERS && (size < MIN_PLAYERS || waited(anchor) < FILL_WAIT) {
                continue;
            }
            taken[a] = true;
            for &b in &picks {
                taken[b] = true;
            }
            let members: Vec<&Ticket> = std::iter::once(a)
                .chain(picks)
                .map(|i| &self.queue[i])
                .collect();
            groups.push(Group {
                players: members.iter().map(|t| t.player).collect(),
                region: majority_region(&members),
            });
        }
        let mut i = 0;
        self.queue.retain(|_| {
            i += 1;
            !taken[i - 1]
        });
        groups
    }
}

/// The region most of `members` are in; a tie goes to the first member's.
fn majority_region(members: &[&Ticket]) -> String {
    let count = |region: &str| members.iter().filter(|t| t.region == region).count();
    let mut best = &members[0].region;
    for t in members {
        if count(&t.region) > count(best) {
            best = &t.region;
        }
    }
    best.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn ticket(player: PlayerId, rating: f64, region: &str, since: Duration) -> Ticket {
        Ticket {
            player,
            rating,
            region: region.to_string(),
            since,
        }
    }

    fn queue(tickets: &[Ticket]) -> Matcher {
        let mut m = Matcher::default();
        for t in tickets {
            m.add(t.clone());
        }
        m
    }

    #[test]
    fn two_players_wait_for_more_before_starting() {
        let mut m = queue(&[
            ticket(1, 1000.0, "eu", Duration::ZERO),
            ticket(2, 1000.0, "eu", 2 * S),
        ]);
        assert!(m.run(9 * S).is_empty(), "room to fill");
        let groups = m.run(10 * S);
        assert_eq!(groups[0].players, [1, 2]);
        assert!(m.is_empty());
    }

    #[test]
    fn a_full_room_starts_at_once_and_the_ninth_waits() {
        let tickets: Vec<Ticket> = (1..=9)
            .map(|i| ticket(i, 1000.0, "eu", i as u32 * Duration::from_millis(10)))
            .collect();
        let mut m = queue(&tickets);
        let groups = m.run(S);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].players.len(), MAX_PLAYERS);
        assert_eq!(groups[0].players[0], 1, "longest-waiting first");
        assert_eq!(m.len(), 1);
        assert!(m.run(5 * S).is_empty(), "alone");
    }

    #[test]
    fn far_apart_ratings_wait_until_the_window_has_grown() {
        assert_eq!(rating_window(Duration::ZERO), 100.0);
        assert_eq!(rating_window(12 * S), 200.0);
        assert!(rating_window(30 * S).is_infinite());

        let mut m = queue(&[
            ticket(1, 1000.0, "eu", Duration::ZERO),
            ticket(2, 1450.0, "eu", Duration::ZERO),
        ]);
        assert!(m.run(25 * S).is_empty(), "window is ±350");
        assert_eq!(m.run(30 * S)[0].players, [1, 2]);
    }

    #[test]
    fn other_regions_wait_until_both_sides_have_waited() {
        let mut m = queue(&[
            ticket(1, 1000.0, "eu", Duration::ZERO),
            ticket(2, 1000.0, "us", 5 * S),
        ]);
        assert!(m.run(20 * S).is_empty(), "the us player has waited 15 s");
        let groups = m.run(25 * S);
        assert_eq!(groups[0].players, [1, 2]);
        assert_eq!(groups[0].region, "eu", "a tie goes to the longest-waiting");
    }

    #[test]
    fn the_same_region_comes_first_then_the_closest_rating() {
        let mut tickets = vec![ticket(1, 1000.0, "eu", Duration::ZERO)];
        // A perfect rating match in another region.
        tickets.push(ticket(2, 1000.0, "us", Duration::ZERO));
        // Seven players here, slightly further off.
        for i in 3..=9 {
            tickets.push(ticket(i, 1000.0 + i as f64, "eu", Duration::ZERO));
        }
        let mut m = queue(&tickets);
        let groups = m.run(25 * S);
        assert!(!groups[0].players.contains(&2), "{groups:?}");
        assert_eq!(groups[0].players.len(), MAX_PLAYERS);

        let mut m = queue(&[
            ticket(1, 1000.0, "eu", Duration::ZERO),
            ticket(2, 1090.0, "eu", Duration::ZERO),
            ticket(3, 1010.0, "eu", Duration::ZERO),
        ]);
        assert_eq!(m.run(10 * S)[0].players, [1, 3, 2]);
    }

    #[test]
    fn the_room_goes_where_most_players_are() {
        let mut m = queue(&[
            ticket(1, 1000.0, "eu", Duration::ZERO),
            ticket(2, 1000.0, "us", Duration::ZERO),
            ticket(3, 1000.0, "us", Duration::ZERO),
        ]);
        assert_eq!(m.run(20 * S)[0].region, "us");
    }

    #[test]
    fn leaving_the_queue_and_queueing_again() {
        let mut m = queue(&[
            ticket(1, 1000.0, "eu", Duration::ZERO),
            ticket(2, 1000.0, "eu", Duration::ZERO),
        ]);
        assert!(m.remove(2));
        assert!(!m.remove(2));
        assert!(m.run(60 * S).is_empty());
        m.add(ticket(1, 1000.0, "eu", 50 * S));
        assert_eq!(m.len(), 1, "a second ticket replaces the first");
    }
}
