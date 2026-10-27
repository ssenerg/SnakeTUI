//! Who's online and what they're doing, for friends' lists, and delivering
//! friend requests and invites to players who are online.

use std::collections::HashMap;
use std::sync::Arc;

use actix::prelude::*;
use game_api::{GameId, PlayerId};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::cluster::Cluster;
use crate::cluster::wire::{StatusQuery, StatusReply, decode};

use crate::room::RoomRef;
use crate::store::AccountId;

/// What a player is doing, least busy first. A player signed in more than
/// once shows the busiest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Status {
    Menu,
    SinglePlayer,
    Watching,
    Queue,
    Room,
    Match,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Menu => "online",
            Status::SinglePlayer => "playing single player",
            Status::Watching => "watching a game",
            Status::Queue => "looking for a match",
            Status::Room => "in a private room",
            Status::Match => "in a match",
        }
    }
}

/// What a player is doing, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doing {
    pub status: Status,
    /// The game they have open.
    pub game: GameId,
    /// The room they're playing in, which friends can watch.
    pub room: Option<RoomRef>,
}

impl Doing {
    /// Whether friends can watch them play.
    pub fn watchable(&self) -> Option<RoomRef> {
        self.room
            .filter(|_| matches!(self.status, Status::Room | Status::Match))
    }

    /// The busier of the two, as a player signed in twice shows.
    fn busier(self, other: Doing) -> Doing {
        if other.status > self.status {
            other
        } else {
            self
        }
    }
}

/// An invite to join a private room.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invite {
    pub from: String,
    pub game: GameId,
    pub room: RoomRef,
}

/// News for a signed-in player.
#[derive(Clone, Debug, Message, Serialize, Deserialize)]
#[rtype(result = "()")]
pub enum Notice {
    FriendRequest { from: String },
    NowFriends { with: String },
    Invite(Invite),
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Online {
    pub account: AccountId,
    pub session: PlayerId,
    pub notices: Recipient<Notice>,
    pub status: Status,
    pub game: GameId,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Offline {
    pub account: AccountId,
    pub session: PlayerId,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetStatus {
    pub account: AccountId,
    pub session: PlayerId,
    pub doing: Doing,
}

/// What each of these accounts is doing; offline ones are left out.
#[derive(Message)]
#[rtype(result = "HashMap<AccountId, Doing>")]
pub struct StatusOf(pub Vec<AccountId>);

/// Passes `notice` to every session `to` is signed in on. Answers whether
/// they're online.
#[derive(Message)]
#[rtype(result = "bool")]
pub struct Deliver {
    pub to: AccountId,
    pub notice: Notice,
}

struct Entry {
    session: PlayerId,
    notices: Recipient<Notice>,
    doing: Doing,
}

/// Who's online here. In a cluster it also answers other servers about its
/// players, and takes notices for them: each account online here is
/// listened for on `presence.notice.<account>`.
#[derive(Default)]
pub struct PresenceActor {
    cluster: Option<Arc<Cluster>>,
    online: HashMap<AccountId, Vec<Entry>>,
    /// Notices from other servers, for each account online here.
    listening: HashMap<AccountId, SpawnHandle>,
}

impl PresenceActor {
    pub fn new(cluster: Option<Arc<Cluster>>) -> Self {
        Self {
            cluster,
            ..Self::default()
        }
    }

    fn status(&self, account: AccountId) -> Option<Doing> {
        self.online
            .get(&account)?
            .iter()
            .filter(|e| e.notices.connected())
            .map(|e| e.doing)
            .reduce(Doing::busier)
    }

    fn statuses(&self, accounts: &[AccountId]) -> HashMap<AccountId, Doing> {
        accounts
            .iter()
            .filter_map(|&a| self.status(a).map(|s| (a, s)))
            .collect()
    }

    /// Hands `notice` to every session `to` has here.
    fn deliver_here(&self, to: AccountId, notice: &Notice) -> bool {
        let Some(entries) = self.online.get(&to) else {
            return false;
        };
        let mut delivered = false;
        for e in entries.iter().filter(|e| e.notices.connected()) {
            e.notices.do_send(notice.clone());
            delivered = true;
        }
        delivered
    }

    fn notice_subject(cluster: &Cluster, account: AccountId) -> String {
        cluster.subject(&format!("presence.notice.{account}"))
    }

    fn listen_for(&mut self, account: AccountId, ctx: &mut Context<Self>) {
        let Some(cluster) = self.cluster.clone() else {
            return;
        };
        if self.listening.contains_key(&account) {
            return;
        }
        let subject = Self::notice_subject(&cluster, account);
        let subscribed = async move { cluster.subscribe(subject).await };
        // Holds the place, so a second session doesn't subscribe again.
        let placeholder = ctx.spawn(fut::ready(()));
        self.listening.insert(account, placeholder);
        ctx.spawn(
            subscribed
                .into_actor(self)
                .map(move |subscribed, act, ctx| match subscribed {
                    Ok(notices) if act.online.contains_key(&account) => {
                        let handle = ctx.add_stream(notices);
                        act.listening.insert(account, handle);
                    }
                    Ok(_) => {
                        act.listening.remove(&account);
                    }
                    Err(e) => {
                        act.listening.remove(&account);
                        warn!(error = %e, "couldn't listen for a player's notices");
                    }
                }),
        );
    }
}

impl Actor for PresenceActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let Some(cluster) = self.cluster.clone() else {
            return;
        };
        let subject = cluster.subject("presence.status");
        let subscribed = async move { cluster.subscribe(subject).await };
        ctx.wait(
            subscribed
                .into_actor(self)
                .map(|subscribed, _, ctx| match subscribed {
                    Ok(questions) => {
                        ctx.add_stream(questions);
                    }
                    Err(e) => warn!(error = %e, "couldn't answer other servers about players"),
                }),
        );
    }
}

/// Other servers' questions about players here, and notices for them.
impl StreamHandler<async_nats::Message> for PresenceActor {
    fn handle(&mut self, msg: async_nats::Message, _: &mut Self::Context) {
        let Some(cluster) = &self.cluster else { return };
        let Some(reply) = msg.reply else { return };
        if msg.subject.as_str().ends_with(".presence.status") {
            let Some(StatusQuery { from, accounts }) = decode(&msg.payload) else {
                return;
            };
            if from != cluster.server {
                let answer = StatusReply(self.statuses(&accounts).into_iter().collect());
                cluster.send(reply.to_string(), &answer);
            }
            return;
        }
        let account = msg
            .subject
            .as_str()
            .rsplit('.')
            .next()
            .and_then(|a| a.parse().ok());
        if let (Some(account), Some(notice)) = (account, decode::<Notice>(&msg.payload))
            && self.deliver_here(account, &notice)
        {
            cluster.send(reply.to_string(), &true);
        }
    }

    // A subscription ends when NATS closes; the actor carries on.
    fn finished(&mut self, _: &mut Self::Context) {}
}

impl Handler<Online> for PresenceActor {
    type Result = ();

    fn handle(&mut self, msg: Online, ctx: &mut Self::Context) {
        let entries = self.online.entry(msg.account).or_default();
        entries.retain(|e| e.session != msg.session && e.notices.connected());
        entries.push(Entry {
            session: msg.session,
            notices: msg.notices,
            doing: Doing {
                status: msg.status,
                game: msg.game,
                room: None,
            },
        });
        self.listen_for(msg.account, ctx);
    }
}

impl Handler<Offline> for PresenceActor {
    type Result = ();

    fn handle(&mut self, msg: Offline, ctx: &mut Self::Context) {
        if let Some(entries) = self.online.get_mut(&msg.account) {
            entries.retain(|e| e.session != msg.session);
            if entries.is_empty() {
                self.online.remove(&msg.account);
                if let Some(listening) = self.listening.remove(&msg.account) {
                    ctx.cancel_future(listening);
                }
            }
        }
    }
}

impl Handler<SetStatus> for PresenceActor {
    type Result = ();

    fn handle(&mut self, msg: SetStatus, _: &mut Self::Context) {
        if let Some(entry) = self
            .online
            .get_mut(&msg.account)
            .and_then(|entries| entries.iter_mut().find(|e| e.session == msg.session))
        {
            entry.doing = msg.doing;
        }
    }
}

impl Handler<StatusOf> for PresenceActor {
    type Result = ResponseFuture<HashMap<AccountId, Doing>>;

    fn handle(&mut self, StatusOf(accounts): StatusOf, _: &mut Self::Context) -> Self::Result {
        let mut statuses = self.statuses(&accounts);
        let Some(cluster) = self.cluster.clone() else {
            return Box::pin(std::future::ready(statuses));
        };
        Box::pin(async move {
            let question = StatusQuery {
                from: cluster.server,
                accounts,
            };
            let answers: Vec<StatusReply> = cluster
                .ask_all(cluster.subject("presence.status"), &question)
                .await;
            for (account, doing) in answers.into_iter().flat_map(|a| a.0) {
                let busiest = statuses.entry(account).or_insert(doing);
                *busiest = busiest.busier(doing);
            }
            statuses
        })
    }
}

impl Handler<Deliver> for PresenceActor {
    type Result = ResponseFuture<bool>;

    fn handle(&mut self, msg: Deliver, _: &mut Self::Context) -> Self::Result {
        let Some(cluster) = self.cluster.clone() else {
            let delivered = self.deliver_here(msg.to, &msg.notice);
            return Box::pin(std::future::ready(delivered));
        };
        // Through NATS even for players here: whichever servers have the
        // account online deliver it, and the first one to answer says so.
        let subject = Self::notice_subject(&cluster, msg.to);
        Box::pin(async move {
            cluster
                .request::<bool>(subject, &msg.notice)
                .await
                .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Inbox(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl Actor for Inbox {
        type Context = Context<Self>;
    }

    impl Handler<Notice> for Inbox {
        type Result = ();

        fn handle(&mut self, notice: Notice, _: &mut Self::Context) {
            let text = match notice {
                Notice::FriendRequest { from } => format!("request from {from}"),
                Notice::NowFriends { with } => format!("friends with {with}"),
                Notice::Invite(i) => format!("invite from {}", i.from),
            };
            self.0.lock().unwrap().push(text);
        }
    }

    #[actix_rt::test]
    async fn statuses_and_notices_follow_every_session() {
        let presence = PresenceActor::default().start();
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let inbox = Inbox(std::sync::Arc::clone(&got)).start();
        for (session, status) in [(1, Status::Menu), (2, Status::Match)] {
            presence
                .send(Online {
                    account: 7,
                    session,
                    notices: inbox.clone().recipient(),
                    status,
                    game: GameId::Snake,
                })
                .await
                .unwrap();
        }
        let statuses = presence.send(StatusOf(vec![7, 8])).await.unwrap();
        assert_eq!(
            statuses.get(&7).map(|d| d.status),
            Some(Status::Match),
            "the busiest session"
        );
        assert_eq!(statuses.get(&8), None, "offline");

        let delivered = presence
            .send(Deliver {
                to: 7,
                notice: Notice::FriendRequest { from: "bob".into() },
            })
            .await
            .unwrap();
        assert!(delivered);
        presence
            .send(Offline {
                account: 7,
                session: 2,
            })
            .await
            .unwrap();
        let room = RoomRef { server: 0, room: 3 };
        presence
            .send(SetStatus {
                account: 7,
                session: 1,
                doing: Doing {
                    status: Status::Room,
                    game: GameId::Snake,
                    room: Some(room),
                },
            })
            .await
            .unwrap();
        let statuses = presence.send(StatusOf(vec![7])).await.unwrap();
        assert_eq!(statuses[&7].watchable(), Some(room), "in a room to watch");
        presence
            .send(SetStatus {
                account: 7,
                session: 1,
                doing: Doing {
                    status: Status::Watching,
                    game: GameId::Snake,
                    room: Some(room),
                },
            })
            .await
            .unwrap();
        let statuses = presence.send(StatusOf(vec![7])).await.unwrap();
        assert_eq!(statuses[&7].status, Status::Watching);
        assert_eq!(statuses[&7].watchable(), None, "watchers aren't watched");
        presence
            .send(Offline {
                account: 7,
                session: 1,
            })
            .await
            .unwrap();
        let nobody = presence
            .send(Deliver {
                to: 7,
                notice: Notice::NowFriends { with: "bob".into() },
            })
            .await
            .unwrap();
        assert!(!nobody);
        // Two sessions, one notice each.
        assert_eq!(got.lock().unwrap().len(), 2);
    }
}
