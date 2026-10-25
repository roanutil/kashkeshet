//! Turning the events part's notices into the live feed's events, for both
//! sources.

use super::source::{Joined, LiveEvent, Message, NewAgent, NewChannel};
use crate::events::Notice;

pub fn live_event(notice: Notice) -> LiveEvent {
    match notice {
        Notice::Message(m) => LiveEvent::Message(Message {
            id: m.id,
            channel_id: m.channel_id,
            channel_name: m.channel_name,
            agent: m.agent,
            body: m.body,
            created_at: m.created_at,
            run_id: m.run_id,
        }),
        Notice::Agent(a) => LiveEvent::Agent(NewAgent {
            name: a.name,
            created_at: a.created_at,
        }),
        Notice::Channel(c) => LiveEvent::Channel(NewChannel {
            id: c.id,
            name: c.name,
            open: c.open,
            created_by: c.created_by,
            member_count: c.member_count,
        }),
        Notice::Joined(j) => LiveEvent::Joined(Joined {
            agent: j.agent,
            channel_id: j.channel_id,
            channel_name: j.channel_name,
        }),
        Notice::Missed(n) => LiveEvent::Missed(n),
    }
}
