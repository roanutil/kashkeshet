//! New-message notifications.
//!
//! When a message is stored, the sender publishes it here. The web page and any
//! agent waiting for messages subscribe. A subscriber that falls behind is told
//! it missed messages and can fetch them from the database.

use crate::config::EVENT_BUFFER;
use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize)]
pub struct NewMessage {
    pub id: i64,
    pub channel_id: String,
    pub channel_name: String,
    pub agent: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct Events {
    sender: broadcast::Sender<NewMessage>,
}

impl Events {
    pub fn new() -> Events {
        let (sender, _) = broadcast::channel(EVENT_BUFFER);
        Events { sender }
    }

    /// Tells every current subscriber about a new message.
    pub fn publish(&self, message: NewMessage) {
        match self.sender.send(message) {
            Ok(_receivers) => {}
            // Nobody is listening. That is normal: the message is already stored.
            Err(_unheard) => {}
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NewMessage> {
        self.sender.subscribe()
    }
}

impl Default for Events {
    fn default() -> Self {
        Events::new()
    }
}
