//! Who's online and what they're doing, for friends' lists, and delivering
//! friend requests and invites to players who are online.

use std::collections::HashMap;

use actix::prelude::*;
use snake_core::round::PlayerId;

use crate::room::RoomActor;
use crate::store::AccountId;

/// What a player is doing, least busy first. A player signed in more than
/// once shows the busiest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Menu,
    SinglePlayer,
    Queue,
    Room,
    Match,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Menu => "online",
            Status::SinglePlayer => "playing single player",
            Status::Queue => "looking for a match",
            Status::Room => "in a private room",
            Status::Match => "in a match",
        }
    }
}

/// An invite to join a private room.
#[derive(Clone)]
pub struct Invite {
    pub from: String,
    pub room: Addr<RoomActor>,
}

/// News for a signed-in player.
#[derive(Clone, Message)]
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
    pub status: Status,
}

/// What each of these accounts is doing; offline ones are left out.
#[derive(Message)]
#[rtype(result = "HashMap<AccountId, Status>")]
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
    status: Status,
}

#[derive(Default)]
pub struct PresenceActor {
    online: HashMap<AccountId, Vec<Entry>>,
}

impl Actor for PresenceActor {
    type Context = Context<Self>;
}

impl PresenceActor {
    fn status(&self, account: AccountId) -> Option<Status> {
        self.online
            .get(&account)?
            .iter()
            .filter(|e| e.notices.connected())
            .map(|e| e.status)
            .max()
    }
}

impl Handler<Online> for PresenceActor {
    type Result = ();

    fn handle(&mut self, msg: Online, _: &mut Self::Context) {
        let entries = self.online.entry(msg.account).or_default();
        entries.retain(|e| e.session != msg.session && e.notices.connected());
        entries.push(Entry {
            session: msg.session,
            notices: msg.notices,
            status: msg.status,
        });
    }
}

impl Handler<Offline> for PresenceActor {
    type Result = ();

    fn handle(&mut self, msg: Offline, _: &mut Self::Context) {
        if let Some(entries) = self.online.get_mut(&msg.account) {
            entries.retain(|e| e.session != msg.session);
            if entries.is_empty() {
                self.online.remove(&msg.account);
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
            entry.status = msg.status;
        }
    }
}

impl Handler<StatusOf> for PresenceActor {
    type Result = MessageResult<StatusOf>;

    fn handle(&mut self, StatusOf(accounts): StatusOf, _: &mut Self::Context) -> Self::Result {
        MessageResult(
            accounts
                .into_iter()
                .filter_map(|a| self.status(a).map(|s| (a, s)))
                .collect(),
        )
    }
}

impl Handler<Deliver> for PresenceActor {
    type Result = bool;

    fn handle(&mut self, msg: Deliver, _: &mut Self::Context) -> bool {
        let Some(entries) = self.online.get(&msg.to) else {
            return false;
        };
        let mut delivered = false;
        for e in entries.iter().filter(|e| e.notices.connected()) {
            e.notices.do_send(msg.notice.clone());
            delivered = true;
        }
        delivered
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
                })
                .await
                .unwrap();
        }
        let statuses = presence.send(StatusOf(vec![7, 8])).await.unwrap();
        assert_eq!(
            statuses.get(&7),
            Some(&Status::Match),
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
        presence
            .send(SetStatus {
                account: 7,
                session: 1,
                status: Status::Queue,
            })
            .await
            .unwrap();
        let statuses = presence.send(StatusOf(vec![7])).await.unwrap();
        assert_eq!(statuses.get(&7), Some(&Status::Queue));
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
