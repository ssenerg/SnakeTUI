//! Who plays whom in quick play: players in the same pool, close in
//! rating, from the same region first, with both limits widening the
//! longer they wait.
//!
//! Plain logic on its own clock, so tests drive it by hand;
//! [`crate::matchmaker::MatchmakerActor`] runs it once a second.

use std::collections::{BTreeMap, HashMap};
use std::ops::RangeInclusive;
use std::time::Duration;

use game_api::PlayerId;
use serde::{Deserialize, Serialize};

use crate::games::{PoolInfo, PoolKey};

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

/// How a pool's matches form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolRules {
    /// How many players a match takes; a full one starts at once.
    pub seats: RangeInclusive<usize>,
    /// A match with fewer than a full room starts once its longest-waiting
    /// player has waited this long.
    pub fill_wait: Duration,
}

impl From<&PoolInfo> for PoolRules {
    fn from(pool: &PoolInfo) -> Self {
        Self {
            seats: pool.seats.clone(),
            fill_wait: pool.fill_wait,
        }
    }
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

/// One pool's queue.
pub struct Matcher {
    rules: PoolRules,
    /// Longest-waiting first.
    queue: Vec<Ticket>,
}

impl Matcher {
    pub fn new(rules: PoolRules) -> Self {
        Self {
            rules,
            queue: Vec::new(),
        }
    }

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
    /// longest-waiting player has waited the pool's fill wait.
    pub fn run(&mut self, now: Duration) -> Vec<Group> {
        let (min, max) = (*self.rules.seats.start(), *self.rules.seats.end());
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
            picks.truncate(max - 1);
            let size = picks.len() + 1;
            if size < max && (size < min || waited(anchor) < self.rules.fill_wait) {
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

/// Every pool's queue. A player waits in one pool at a time.
pub struct Pools {
    pools: BTreeMap<PoolKey, Matcher>,
    /// Which pool each waiting player is in.
    of: HashMap<PlayerId, PoolKey>,
    /// How each pool's matches form; `None` for pools no game here has.
    rules: fn(&PoolKey) -> Option<PoolRules>,
}

/// How many players wait in each pool; a pool nobody waits in is left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting(pub BTreeMap<PoolKey, u32>);

impl Waiting {
    /// Players waiting, in every pool.
    pub fn total(&self) -> usize {
        self.0.values().map(|&n| n as usize).sum()
    }

    /// Players waiting in `pool`.
    pub fn of(&self, pool: &PoolKey) -> usize {
        self.0.get(pool).map_or(0, |&n| n as usize)
    }
}

impl Default for Pools {
    /// The pools of the games this server serves.
    fn default() -> Self {
        Self::with_rules(|pool| pool.find().map(|(_, info)| info.into()))
    }
}

impl Pools {
    pub fn with_rules(rules: fn(&PoolKey) -> Option<PoolRules>) -> Self {
        Self {
            pools: BTreeMap::new(),
            of: HashMap::new(),
            rules,
        }
    }

    /// Queues a player in `pool`, taking them out of any other, and
    /// replacing any ticket they had. Says whether the pool exists.
    pub fn add(&mut self, pool: &PoolKey, ticket: Ticket) -> bool {
        let Some(rules) = (self.rules)(pool) else {
            return false;
        };
        self.remove(ticket.player);
        self.of.insert(ticket.player, pool.clone());
        self.pools
            .entry(pool.clone())
            .or_insert_with(|| Matcher::new(rules))
            .add(ticket);
        true
    }

    pub fn remove(&mut self, player: PlayerId) -> bool {
        let Some(pool) = self.of.remove(&player) else {
            return false;
        };
        self.pools.get_mut(&pool).is_some_and(|m| m.remove(player))
    }

    /// Players waiting, in every pool.
    pub fn len(&self) -> usize {
        self.of.len()
    }

    /// How many wait in each pool.
    pub fn waiting(&self) -> Waiting {
        let counts = self.pools.iter().filter_map(|(pool, matcher)| {
            let n = u32::try_from(matcher.len()).unwrap_or(u32::MAX);
            (n > 0).then(|| (pool.clone(), n))
        });
        Waiting(counts.collect())
    }

    pub fn is_empty(&self) -> bool {
        self.of.is_empty()
    }

    /// Forms every match it can in every pool at time `now`, and takes
    /// those players out of the queue.
    pub fn run(&mut self, now: Duration) -> Vec<(PoolKey, Group)> {
        let mut formed = Vec::new();
        for (pool, matcher) in &mut self.pools {
            for group in matcher.run(now) {
                for player in &group.players {
                    self.of.remove(player);
                }
                formed.push((pool.clone(), group));
            }
        }
        self.pools.retain(|_, m| !m.is_empty());
        formed
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
    use game_api::GameId;

    use super::*;

    const S: Duration = Duration::from_secs(1);
    /// A full Snake room.
    const FULL: usize = 8;

    fn snake() -> PoolRules {
        PoolRules {
            seats: 2..=FULL,
            fill_wait: 10 * S,
        }
    }

    fn ticket(player: PlayerId, rating: f64, region: &str, since: Duration) -> Ticket {
        Ticket {
            player,
            rating,
            region: region.to_string(),
            since,
        }
    }

    fn queue(tickets: &[Ticket]) -> Matcher {
        let mut m = Matcher::new(snake());
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
        assert_eq!(groups[0].players.len(), FULL);
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
        assert_eq!(groups[0].players.len(), FULL);

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

    fn pool(name: &str) -> PoolKey {
        PoolKey {
            game: GameId::Snake,
            pool: name.into(),
        }
    }

    /// Duels take two at once; "ffa" is Snake's; anything else isn't
    /// served.
    fn pools() -> Pools {
        Pools::with_rules(|key| match key.pool.as_str() {
            "duel" => Some(PoolRules {
                seats: 2..=2,
                fill_wait: Duration::ZERO,
            }),
            "ffa" => Some(snake()),
            _ => None,
        })
    }

    #[test]
    fn pools_are_matched_apart_and_a_player_waits_in_one() {
        let mut pools = pools();
        for (player, name) in [(1, "duel"), (2, "ffa"), (3, "duel"), (4, "ffa")] {
            assert!(pools.add(&pool(name), ticket(player, 1000.0, "eu", Duration::ZERO)));
        }
        assert!(!pools.add(&pool("chess"), ticket(5, 1000.0, "eu", Duration::ZERO)));
        assert_eq!(pools.len(), 4);
        let formed = pools.run(S);
        assert_eq!(formed.len(), 1, "the ffa pool waits to fill: {formed:?}");
        assert_eq!(formed[0].0, pool("duel"));
        assert_eq!(formed[0].1.players, [1, 3]);
        assert_eq!(pools.len(), 2);

        // Queueing in another pool leaves the first.
        assert!(pools.add(&pool("duel"), ticket(2, 1000.0, "eu", S)));
        assert_eq!(pools.len(), 2);
        // Each is counted where they wait.
        let waiting = pools.waiting();
        assert_eq!(
            (waiting.of(&pool("duel")), waiting.of(&pool("ffa"))),
            (1, 1)
        );
        assert_eq!((waiting.of(&pool("chess")), waiting.total()), (0, 2));
        assert!(pools.run(20 * S).is_empty(), "alone in each pool");
        assert!(pools.remove(4));
        assert!(!pools.remove(4));
        assert_eq!(pools.len(), 1);
        let waiting = pools.waiting();
        assert_eq!(
            waiting.0.len(),
            1,
            "an empty pool isn't listed: {waiting:?}"
        );
    }
}
