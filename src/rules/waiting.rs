//! Open waits and the short grace after normal completion. Never persisted.
use super::*;
use chrono::{DateTime, Utc};
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

#[derive(Default)]
pub struct Waiting(Mutex<State>);
#[derive(Default)]
struct State {
    next: u64,
    calls: HashMap<u64, Call>,
}
struct Call {
    agent: String,
    channels: Option<Vec<String>>,
    until: Option<DateTime<Utc>>,
}
pub(super) struct Guard<'a> {
    waiting: &'a Waiting,
    id: u64,
    completed: bool,
}
impl Waiting {
    pub(super) fn start(
        &self,
        agent: &str,
        channels: Option<Vec<String>>,
        now: DateTime<Utc>,
    ) -> Guard<'_> {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state
            .calls
            .retain(|_, c| c.until.is_none_or(|end| c.agent != agent && end > now));
        state.next += 1;
        let id = state.next;
        state.calls.insert(
            id,
            Call {
                agent: agent.into(),
                channels,
                until: None,
            },
        );
        Guard {
            waiting: self,
            id,
            completed: false,
        }
    }
    fn filters(&self, agent: &str, now: DateTime<Utc>) -> Vec<Option<Vec<String>>> {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state
            .calls
            .retain(|_, c| c.until.is_none_or(|end| end > now));
        state
            .calls
            .values()
            .filter(|c| c.agent == agent)
            .map(|c| c.channels.clone())
            .collect()
    }
}
impl Guard<'_> {
    pub(super) fn finish(mut self, until: DateTime<Utc>) {
        let mut state = self.waiting.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(call) = state.calls.get_mut(&self.id) {
            call.until = Some(until);
        }
        self.completed = true;
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.waiting
                .0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .calls
                .remove(&self.id);
        }
    }
}
impl Rules {
    /// Adds only channels the viewer can see, still joined by the waiting agent.
    pub(super) fn observed_agent(
        &self,
        store: &dyn Reads,
        record: AgentRecord,
        viewer: Option<&str>,
        channel: Option<&str>,
    ) -> Result<AgentInfo> {
        let filters = self
            .inner
            .deps
            .waiting
            .filters(&record.name, self.inner.deps.clock.now());
        let mut ids = BTreeSet::new();
        for filter in filters {
            let channels = match filter {
                Some(ids) => ids,
                None => store.all_channel_ids(Some(&record.name))?,
            };
            for id in channels {
                let Some(facts) = store.channel(&id)? else {
                    continue;
                };
                if !store.is_member(&id, &record.name)? {
                    continue;
                }
                let visible = match viewer {
                    None => true,
                    Some(who) => {
                        access::decide(true, facts.open, store.is_member(&id, who)?, Need::See)
                            == Decision::Allowed
                    }
                };
                if visible {
                    ids.insert(id);
                }
            }
        }
        let mut info = AgentInfo::from(record);
        info.waiting_channels = ids.into_iter().collect();
        info.listening = channel.map_or(!info.waiting_channels.is_empty(), |id| {
            info.waiting_channels.iter().any(|c| c == id)
        });
        Ok(info)
    }
}
