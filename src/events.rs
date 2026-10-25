//! New-message notifications.
//!
//! When a message is stored, the sender publishes it here. The web page and any
//! agent waiting for messages subscribe. A subscriber that falls behind is told
//! it missed messages and can fetch them from the database.
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

#[derive(Clone)]
pub struct Events {
    sender: broadcast::Sender<NewMessage>,
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
        match self.sender.send(message) {
            Ok(_receivers) => {}
            // Nobody is listening. That is normal: the message is already stored.
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
    /// The subscriber fell behind and missed this many messages. They are in
    /// the database.
    Missed(u64),
}

/// One subscriber's view of new messages.
pub struct Subscription {
    receiver: broadcast::Receiver<NewMessage>,
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
            Ok(message) => Some(Notice::Message(message)),
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
                Notice::Missed(n) => panic!("missed {n}"),
            })
            .collect()
            .await;
        assert_eq!(ids, vec![1, 2]);
    }
}
