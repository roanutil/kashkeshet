//! Notifications of what happens: new messages, and new agents, channels and
//! channel members.
//!
//! When a message is stored, the sender publishes it here, and the rules do
//! the same when an agent registers, a channel is created, or an agent joins
//! one. The web page and any agent waiting for messages subscribe; waiting
//! agents look only at messages. A subscriber that falls behind is told it
//! missed some and can fetch them from the database.
//!
//! Tokio's broadcast channel does the work, but stays inside this module.

use crate::config::EVENT_BUFFER;
use crate::shutdown::Shutdown;
use futures::Stream;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

#[derive(Clone, Debug, Serialize)]
pub struct NewMessage {
    pub id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    pub body: String,
    pub created_at: String,
    /// The server run that stored the message.
    pub run_id: Option<i64>,
}

/// An agent registered.
#[derive(Clone, Debug, Serialize)]
pub struct NewAgent {
    pub name: String,
    pub created_at: String,
}

/// A channel was created. Closed channels are announced too: only the
/// human's view hears these.
#[derive(Clone, Debug, Serialize)]
pub struct NewChannel {
    pub id: String,
    pub name: String,
    pub open: bool,
    pub created_by: String,
    pub created_at: String,
    /// The creator and anyone added at creation.
    pub member_count: i64,
}

/// An agent joined a channel it was not in, or was added to it.
#[derive(Clone, Debug, Serialize)]
pub struct Joined {
    pub agent: String,
    pub channel_id: String,
    pub channel_name: String,
    /// The channel's members afterwards.
    pub member_count: i64,
}

/// An agent left a channel.
#[derive(Clone, Debug, Serialize)]
pub struct Left {
    pub agent: String,
    pub channel_id: String,
    pub channel_name: String,
    /// The channel's members afterwards.
    pub member_count: i64,
}

/// Everything that can be published.
#[derive(Clone, Debug)]
enum Published {
    Message(NewMessage),
    Agent(NewAgent),
    Channel(NewChannel),
    Joined(Joined),
    Left(Left),
}

#[derive(Clone)]
pub struct Events {
    sender: broadcast::Sender<Published>,
    shutdown: Option<Shutdown>,
}

impl Events {
    pub fn new() -> Events {
        Events::with_buffer(EVENT_BUFFER)
    }

    /// Keeps `buffer` notices for a slow subscriber before it starts missing them.
    pub fn with_buffer(buffer: usize) -> Events {
        let (sender, _) = broadcast::channel(buffer);
        Events {
            sender,
            shutdown: None,
        }
    }

    /// Ends subscriptions when the server stops, even while publishers exist.
    pub fn until_stopped(mut self, shutdown: Shutdown) -> Self {
        self.shutdown = Some(shutdown);
        self
    }

    /// Tells every current subscriber about a new message.
    pub fn publish(&self, message: NewMessage) {
        self.send(Published::Message(message));
    }

    /// Tells every current subscriber that an agent registered.
    pub fn publish_agent(&self, agent: NewAgent) {
        self.send(Published::Agent(agent));
    }

    /// Tells every current subscriber that a channel was created.
    pub fn publish_channel(&self, channel: NewChannel) {
        self.send(Published::Channel(channel));
    }

    /// Tells every current subscriber that an agent joined a channel.
    pub fn publish_joined(&self, joined: Joined) {
        self.send(Published::Joined(joined));
    }

    /// Tells every current subscriber that an agent left a channel.
    pub fn publish_left(&self, left: Left) {
        self.send(Published::Left(left));
    }

    fn send(&self, published: Published) {
        match self.sender.send(published) {
            Ok(_receivers) => {}
            // Nobody is listening. That is normal: what happened is already stored.
            Err(_unheard) => {}
        }
    }

    /// Hears about every message published from now on.
    pub fn subscribe(&self) -> Subscription {
        Subscription {
            receiver: self.sender.subscribe(),
            shutdown: self.shutdown.clone(),
        }
    }
}

/// What a subscriber hears.
#[derive(Clone, Debug)]
pub enum Notice {
    /// A new message.
    Message(NewMessage),
    /// A new agent.
    Agent(NewAgent),
    /// A new channel.
    Channel(NewChannel),
    /// An agent joined a channel.
    Joined(Joined),
    /// An agent left a channel.
    Left(Left),
    /// The subscriber fell behind and missed this many notices. What they
    /// were about is in the database.
    Missed(u64),
}

/// One subscriber's view of new messages.
pub struct Subscription {
    receiver: broadcast::Receiver<Published>,
    shutdown: Option<Shutdown>,
}

impl Subscription {
    /// The next notice, or `None` once nothing more can be published (every
    /// `Events` handle is gone), or the server's shutdown signal is stopped.
    pub async fn next(&mut self) -> Option<Notice> {
        let received = tokio::select! {
            biased;
            () = async {
                match &self.shutdown {
                    Some(shutdown) => shutdown.stopped().await,
                    None => std::future::pending().await,
                }
            } => return None,
            received = self.receiver.recv() => received,
        };
        match received {
            Ok(Published::Message(message)) => Some(Notice::Message(message)),
            Ok(Published::Agent(agent)) => Some(Notice::Agent(agent)),
            Ok(Published::Channel(channel)) => Some(Notice::Channel(channel)),
            Ok(Published::Joined(joined)) => Some(Notice::Joined(joined)),
            Ok(Published::Left(left)) => Some(Notice::Left(left)),
            Err(RecvError::Lagged(n)) => Some(Notice::Missed(n)),
            Err(RecvError::Closed) => None,
        }
    }

    /// The notices as a stream, which ends when `next` would return `None`.
    pub fn into_stream(self) -> impl Stream<Item = Notice> {
        futures::stream::unfold(self, |mut subscription| async move {
            let notice = subscription.next().await?;
            Some((notice, subscription))
        })
    }
}

impl Default for Events {
    fn default() -> Self {
        Events::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn shutdown_ends_pending_and_new_subscriptions_with_publishers_alive() {
        let shutdown = Shutdown::new();
        let events = Events::new().until_stopped(shutdown.clone());
        let mut sub = events.subscribe();
        let next = sub.next();
        tokio::pin!(next);
        assert!(futures::poll!(&mut next).is_pending());
        shutdown.stop();
        assert!(next.await.is_none());
        events.publish(message(1));
        assert!(events.subscribe().next().await.is_none());
    }

    fn message(id: i64) -> NewMessage {
        NewMessage {
            id,
            channel_id: "ch_test".into(),
            channel_name: "test".into(),
            agent: "a".into(),
            body: "hi".into(),
            created_at: "2026-10-23T14:00:00Z".into(),
            run_id: None,
        }
    }

    fn id(notice: Option<Notice>) -> Option<i64> {
        match notice {
            Some(Notice::Message(m)) => Some(m.id),
            other => panic!("expected a message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_subscriber_hears_every_kind_in_order() {
        let events = Events::new();
        let mut sub = events.subscribe();
        events.publish_agent(NewAgent {
            name: "amber-falcon".into(),
            created_at: "2026-10-23T14:00:00Z".into(),
        });
        events.publish_channel(NewChannel {
            id: "ch_test".into(),
            name: "test".into(),
            open: false,
            created_by: "amber-falcon".into(),
            created_at: "2026-10-23T14:00:00Z".into(),
            member_count: 1,
        });
        events.publish_joined(Joined {
            agent: "bold-panther".into(),
            channel_id: "ch_test".into(),
            channel_name: "test".into(),
            member_count: 2,
        });
        events.publish_left(Left {
            agent: "bold-panther".into(),
            channel_id: "ch_test".into(),
            channel_name: "test".into(),
            member_count: 1,
        });
        events.publish(message(1));
        assert!(matches!(sub.next().await, Some(Notice::Agent(a)) if a.name == "amber-falcon"));
        assert!(
            matches!(sub.next().await, Some(Notice::Channel(c)) if c.id == "ch_test" && !c.open)
        );
        assert!(
            matches!(sub.next().await, Some(Notice::Joined(j)) if j.agent == "bold-panther" && j.member_count == 2)
        );
        assert!(matches!(sub.next().await, Some(Notice::Left(l)) if l.member_count == 1));
        assert_eq!(id(sub.next().await), Some(1));
    }

    #[tokio::test]
    async fn a_subscriber_hears_messages_in_order() {
        let events = Events::new();
        let mut sub = events.subscribe();
        events.publish(message(1));
        events.publish(message(2));
        assert_eq!(id(sub.next().await), Some(1));
        assert_eq!(id(sub.next().await), Some(2));
    }

    #[tokio::test]
    async fn a_slow_subscriber_is_told_how_many_it_missed() {
        let events = Events::with_buffer(2);
        let mut sub = events.subscribe();
        for i in 1..=5 {
            events.publish(message(i));
        }
        assert!(matches!(sub.next().await, Some(Notice::Missed(3))));
        assert_eq!(id(sub.next().await), Some(4));
    }

    #[tokio::test]
    async fn the_subscription_ends_when_events_are_gone() {
        let events = Events::new();
        let mut sub = events.subscribe();
        events.publish(message(1));
        drop(events);
        assert_eq!(id(sub.next().await), Some(1));
        assert!(sub.next().await.is_none());
    }

    #[tokio::test]
    async fn the_stream_gives_the_same_notices_and_then_ends() {
        let events = Events::new();
        let stream = events.subscribe().into_stream();
        events.publish(message(1));
        events.publish(message(2));
        drop(events);
        let ids: Vec<i64> = stream
            .map(|notice| match notice {
                Notice::Message(m) => m.id,
                other => panic!("expected a message, got {other:?}"),
            })
            .collect()
            .await;
        assert_eq!(ids, vec![1, 2]);
    }
}
