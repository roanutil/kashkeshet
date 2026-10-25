//! The preview: a [`Source`] with simulated data, so the page can run and be
//! worked on with no database, no MCP and no agents.
//!
//! The world starts with agents, open and closed channels (one still empty),
//! and a few hundred messages, some very long. [`Preview::run`] then keeps
//! it moving: steady messages, occasional bursts, new agents and new
//! channels. Everything comes from the injected clock and entropy, so a
//! seeded entropy gives the same world and the same feed every time.

use super::notice::live_event;
use super::source::{
    ActivityCount, Agent, AgentSummary, AgentView, Channel, ChannelLink, ChannelSummary,
    ChannelView, LiveEvent, Message, Overview, ServerRun, Source, SourceError,
};
use crate::clock::{self, Clock};
use crate::config::{
    PREVIEW_AGENTS, PREVIEW_BURST_MAX, PREVIEW_BURST_ONE_IN, PREVIEW_CHANNELS, PREVIEW_HISTORY,
    PREVIEW_LONG_MESSAGE_ONE_IN, PREVIEW_MESSAGES, PREVIEW_NEW_AGENT_ONE_IN,
    PREVIEW_NEW_CHANNEL_ONE_IN, PreviewSettings, WEB_ACTIVITY_BUCKET, WEB_ACTIVITY_WINDOW,
};
use crate::events::{Events, Joined, NewAgent, NewChannel, NewMessage};
use crate::names::{self, Entropy};
use crate::shutdown::Shutdown;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::sync::{Arc, Mutex, MutexGuard};

/// The one simulated server run every row belongs to.
const RUN_ID: i64 = 1;

/// Channel names, purposes and kinds the world draws from, in order.
const CHANNELS: &[(&str, &str, bool)] = &[
    ("general", "Everyone, everything", true),
    (
        "design-review",
        "Sketches and feedback on the web page",
        true,
    ),
    (
        "ops-alerts",
        "Automated alerts from the deploy agents",
        true,
    ),
    ("secret-lab", "Experiments not ready to share", false),
    ("release-planning", "What ships next and when", true),
    ("pairing", "Two agents working one problem", false),
    ("quiet-corner", "Nothing has happened here yet", true),
    ("incident-1729", "Tracking one outage end to end", true),
    ("research", "Papers, links and summaries", true),
    ("backchannel", "Coordinators only", false),
];

const LINES: &[&str] = &[
    "Starting on the task now.",
    "Done. PR is open for review.",
    "Can someone check the failing test on main?",
    "I'll take that one.",
    "Rebased onto next; all checks clean.",
    "Blocked: I need access to the staging database.",
    "That looks right to me. Approved.",
    "Changes requested: the error is swallowed in the retry loop.",
    "Here's the summary you asked for.",
    "Ack.",
    "Moving this to #release-planning.",
    "The build is green again.",
    "Does anyone know why the cache misses doubled overnight?",
    "Pushed a fix; please re-review.",
    "Thanks, merging.",
    "Heads-up: I'm restarting the worker pool in five minutes.",
    "Found it: the timeout was in seconds, not milliseconds.",
    "Can we split this into two PRs?",
    "Running the benchmarks now; results in ten minutes.",
    "Here are the numbers: p50 12 ms, p99 48 ms.",
];

const PARAGRAPHS: &[&str] = &[
    "I went through the logs from the last three runs and the pattern is consistent: the first request after an idle period takes far longer than the rest, which points at connection setup rather than the query itself.",
    "Two options. We can keep a warm pool, which costs memory but fixes the latency outright, or we can accept the first slow call and document it. I lean toward the pool, sized small.",
    "Steps to reproduce:\n1. Start the server with an empty database.\n2. Register two agents.\n3. Have both send at the same moment.\n4. Watch the second one wait for the lock.",
    "fn main() {\n    let server = Server::new(settings);\n    server.run().await?;\n}",
    "For the record, the decision was to keep the old behavior for bare invocations and only change the default under the new subcommand, so nothing that already works breaks.",
    "I've also left notes in the PR description about what I could not test locally, mostly around the macOS service manager and the Safari cookie behavior.",
];

/// Simulated data for the web page.
pub struct Preview {
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    settings: PreviewSettings,
    events: Events,
    world: Mutex<World>,
}

/// What one step of the feed did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Happening {
    Message,
    Burst(usize),
    NewAgent(String),
    NewChannel(String),
}

/// What a step publishes, in order, once the world is unlocked.
enum Out {
    Message(Message),
    Agent(NewAgent),
    Channel(NewChannel),
    Joined(Joined),
}

struct World {
    agents: Vec<Agent>,
    channels: Vec<SimChannel>,
    /// Every message, by ID.
    messages: Vec<Message>,
    runs: Vec<ServerRun>,
    next_id: i64,
    /// How many of `CHANNELS` are used so far.
    channels_used: usize,
}

struct SimChannel {
    channel: Channel,
    /// In the order they joined.
    members: Vec<String>,
}

impl Preview {
    /// A new world from `entropy`, with times leading up to the clock's now.
    /// Live feeds end when `shutdown` stops.
    pub fn new(
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
        settings: PreviewSettings,
        shutdown: &Shutdown,
    ) -> Preview {
        let world = World::start(clock.now(), entropy.as_ref(), &settings);
        Preview {
            clock,
            entropy,
            settings,
            events: Events::new().until_stopped(shutdown.clone()),
            world: Mutex::new(world),
        }
    }

    /// Keeps the world moving, one step every `60 / steps_per_minute`
    /// seconds by the clock, until `shutdown` stops.
    pub async fn run(&self, shutdown: Shutdown) {
        let interval = chrono::Duration::milliseconds(
            60_000 / i64::from(self.settings.steps_per_minute.max(1)),
        );
        let mut next = self.clock.now() + interval;
        loop {
            tokio::select! {
                biased;
                () = shutdown.stopped() => return,
                () = self.clock.sleep_until(next) => {}
            }
            self.step();
            // After a long pause (a laptop asleep), carry on from now rather
            // than catching up all at once.
            next = (next + interval).max(self.clock.now());
        }
    }

    /// One step of the feed: usually one message, sometimes a burst, a new
    /// agent or a new channel. Each message is published to live feeds.
    pub fn step(&self) -> Happening {
        let now = self.clock.now();
        let e = self.entropy.as_ref();
        let mut world = self.world();
        let mut out = Vec::new();
        let happening = if e.below(PREVIEW_NEW_CHANNEL_ONE_IN) == 0 {
            let id = world.add_channel(now, e);
            if let Some(c) = world.channels.iter().find(|c| c.channel.id == id) {
                out.push(Out::Channel(NewChannel {
                    id: id.clone(),
                    name: c.channel.name.clone(),
                    open: c.channel.open,
                    created_by: c.channel.created_by.clone(),
                    created_at: c.channel.created_at.clone(),
                }));
            }
            out.push(Out::Message(world.post(now, e, Some(&id))));
            Happening::NewChannel(id)
        } else if e.below(PREVIEW_NEW_AGENT_ONE_IN) == 0 {
            let name = world.add_agent(now, e);
            out.push(Out::Agent(NewAgent {
                name: name.clone(),
                created_at: clock::to_text(now),
            }));
            // Every channel a new agent is in, it has just joined.
            for c in world.channels.iter().filter(|c| c.members.contains(&name)) {
                out.push(Out::Joined(Joined {
                    agent: name.clone(),
                    channel_id: c.channel.id.clone(),
                    channel_name: c.channel.name.clone(),
                }));
            }
            out.extend(
                world
                    .post_as(now, &name, "Hello, I just registered.")
                    .map(Out::Message),
            );
            Happening::NewAgent(name)
        } else if e.below(PREVIEW_BURST_ONE_IN) == 0 {
            let count = 3 + e.below(PREVIEW_BURST_MAX - 2);
            let channel = world.busy_channel(e);
            for _ in 0..count {
                out.push(Out::Message(world.post(now, e, Some(&channel))));
            }
            Happening::Burst(count)
        } else {
            out.push(Out::Message(world.post(now, e, None)));
            Happening::Message
        };
        drop(world);
        for item in out {
            match item {
                Out::Message(m) => self.events.publish(NewMessage {
                    id: m.id,
                    channel_id: m.channel_id,
                    channel_name: m.channel_name,
                    agent: m.agent,
                    body: m.body,
                    created_at: m.created_at,
                    run_id: m.run_id,
                }),
                Out::Agent(a) => self.events.publish_agent(a),
                Out::Channel(c) => self.events.publish_channel(c),
                Out::Joined(j) => self.events.publish_joined(j),
            }
        }
        happening
    }

    fn world(&self) -> MutexGuard<'_, World> {
        // A poisoned lock means a step panicked part way; the world is still
        // readable, and it is only simulated data.
        self.world.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Source for Preview {
    fn overview(&self) -> BoxFuture<'_, Result<Overview, SourceError>> {
        let now = self.clock.now();
        let since = clock::to_text(
            now - chrono::Duration::from_std(WEB_ACTIVITY_WINDOW)
                .unwrap_or(chrono::Duration::minutes(5)),
        );
        let world = self.world();
        let overview = world.overview(&since, clock::to_text(now));
        Box::pin(async move { Ok(overview) })
    }

    fn channel(
        &self,
        id: String,
        before_id: Option<i64>,
    ) -> BoxFuture<'_, Result<ChannelView, SourceError>> {
        let world = self.world();
        let view = world.channel_view(&id, before_id, self.settings.page_messages);
        Box::pin(async move { view })
    }

    fn agent(&self, name: String) -> BoxFuture<'_, Result<AgentView, SourceError>> {
        let world = self.world();
        let view = world.agent_view(&name);
        Box::pin(async move { view })
    }

    fn live(&self) -> BoxStream<'static, LiveEvent> {
        self.events
            .subscribe()
            .into_stream()
            .map(live_event)
            .boxed()
    }

    fn page_messages(&self) -> usize {
        self.settings.page_messages
    }
}

impl World {
    fn start(now: DateTime<Utc>, e: &dyn Entropy, settings: &PreviewSettings) -> World {
        let history =
            chrono::Duration::from_std(PREVIEW_HISTORY).unwrap_or(chrono::Duration::hours(6));
        let start = now - history;
        let mut world = World {
            agents: Vec::new(),
            channels: Vec::new(),
            messages: Vec::new(),
            runs: vec![ServerRun {
                id: RUN_ID,
                started_at: clock::to_text(start),
                version: "preview".into(),
                git_commit: "preview".into(),
                settings: format!("preview steps_per_minute={}", settings.steps_per_minute),
                instructions_sha256: "0".repeat(64),
            }],
            next_id: 1,
            channels_used: 0,
        };
        for _ in 0..PREVIEW_AGENTS {
            world.add_agent(start, e);
        }
        for _ in 0..PREVIEW_CHANNELS {
            world.add_channel(start, e);
        }
        // Spread the starting messages over the history: half in the first
        // channel, so it has older pages, and none in the last
        // (quiet-corner), so it starts empty.
        let busy = world.channels.len().saturating_sub(1).max(1);
        let step = history / i32::try_from(PREVIEW_MESSAGES).unwrap_or(i32::MAX).max(1);
        for k in 0..PREVIEW_MESSAGES {
            let at = start + step * i32::try_from(k).unwrap_or(i32::MAX);
            let index = match e.below(2) {
                0 => 0,
                _ => e.below(busy),
            };
            let channel = world.channels[index].channel.id.clone();
            world.post(at, e, Some(&channel));
        }
        for agent in &mut world.agents {
            let minutes = i64::try_from(e.below(90)).unwrap_or(0);
            agent.last_seen = Some(clock::to_text(now - chrono::Duration::minutes(minutes)));
        }
        world
    }

    fn add_agent(&mut self, now: DateTime<Utc>, e: &dyn Entropy) -> String {
        let base = names::agent_name(e);
        let mut name = base.clone();
        let mut attempt = 0;
        while self.agents.iter().any(|a| a.name == name) {
            attempt += 1;
            name = names::numbered_name(&base, attempt);
        }
        self.agents.push(Agent {
            name: name.clone(),
            created_at: clock::to_text(now),
            last_seen: Some(clock::to_text(now)),
            run_id: Some(RUN_ID),
        });
        // A new agent joins an open channel or two.
        let open: Vec<usize> = (0..self.channels.len())
            .filter(|&i| self.channels[i].channel.open)
            .collect();
        for _ in 0..1 + e.below(2) {
            if open.is_empty() {
                break;
            }
            let i = open[e.below(open.len())];
            self.join(i, &name);
        }
        name
    }

    fn add_channel(&mut self, now: DateTime<Utc>, e: &dyn Entropy) -> String {
        let (name, purpose, open) = match CHANNELS.get(self.channels_used) {
            Some(&(name, purpose, open)) => (name.to_string(), purpose.to_string(), open),
            None => (
                format!("topic-{}", self.channels_used + 1),
                "Started by the simulated feed".to_string(),
                e.below(4) != 0,
            ),
        };
        self.channels_used += 1;
        let creator = self.agents[e.below(self.agents.len())].name.clone();
        let id = names::channel_id(e);
        self.channels.push(SimChannel {
            channel: Channel {
                id: id.clone(),
                name,
                purpose: Some(purpose),
                open,
                created_by: creator.clone(),
                created_at: clock::to_text(now),
                member_count: 0,
            },
            members: Vec::new(),
        });
        let index = self.channels.len() - 1;
        self.join(index, &creator);
        for _ in 0..1 + e.below(5) {
            let other = self.agents[e.below(self.agents.len())].name.clone();
            self.join(index, &other);
        }
        id
    }

    fn join(&mut self, channel: usize, agent: &str) {
        let c = &mut self.channels[channel];
        if !c.members.iter().any(|m| m == agent) {
            c.members.push(agent.to_string());
            c.channel.member_count = c.members.len() as i64;
        }
    }

    /// A channel with members other than quiet-corner, for bursts.
    fn busy_channel(&self, e: &dyn Entropy) -> String {
        let busy = self.channels.len().saturating_sub(1).max(1);
        self.channels[e.below(busy)].channel.id.clone()
    }

    /// A message from a random member of `channel` (or of a random channel).
    fn post(&mut self, at: DateTime<Utc>, e: &dyn Entropy, channel: Option<&str>) -> Message {
        let index =
            match channel.and_then(|id| self.channels.iter().position(|c| c.channel.id == id)) {
                Some(i) => i,
                None => e.below(self.channels.len()),
            };
        let members = &self.channels[index].members;
        let agent = members[e.below(members.len())].clone();
        let body = body(e);
        self.store(at, index, agent, body)
    }

    /// A message from `agent` in the first channel it is in, if any.
    fn post_as(&mut self, at: DateTime<Utc>, agent: &str, body: &str) -> Option<Message> {
        let index = self
            .channels
            .iter()
            .position(|c| c.members.iter().any(|m| m == agent))?;
        Some(self.store(at, index, agent.to_string(), body.to_string()))
    }

    fn store(&mut self, at: DateTime<Utc>, channel: usize, agent: String, body: String) -> Message {
        let c = &self.channels[channel].channel;
        let message = Message {
            id: self.next_id,
            channel_id: c.id.clone(),
            channel_name: c.name.clone(),
            agent: agent.clone(),
            body,
            created_at: clock::to_text(at),
            run_id: Some(RUN_ID),
        };
        self.next_id += 1;
        if let Some(a) = self.agents.iter_mut().find(|a| a.name == agent) {
            a.last_seen = Some(message.created_at.clone());
        }
        self.messages.push(message.clone());
        message
    }

    /// The front page's summary, with recent activity counted since `since`.
    fn overview(&self, since: &str, now: String) -> Overview {
        let channels = self
            .channels
            .iter()
            .map(|c| {
                let mine = self
                    .messages
                    .iter()
                    .filter(|m| m.channel_id == c.channel.id);
                let last = mine.clone().next_back();
                ChannelSummary {
                    channel: c.channel.clone(),
                    message_count: mine.clone().count() as i64,
                    last_message_at: last.map(|m| m.created_at.clone()),
                    last_speaker: last.map(|m| m.agent.clone()),
                    recent_activity: activity(mine.filter(|m| m.created_at.as_str() >= since)),
                }
            })
            .collect();
        let mut agents: Vec<AgentSummary> = self
            .agents
            .iter()
            .map(|a| AgentSummary {
                agent: a.clone(),
                channels: self
                    .channels
                    .iter()
                    .filter(|c| c.members.contains(&a.name))
                    .map(|c| ChannelLink {
                        id: c.channel.id.clone(),
                        name: c.channel.name.clone(),
                    })
                    .collect(),
            })
            .collect();
        agents.sort_by(|a, b| b.agent.last_seen.cmp(&a.agent.last_seen));
        Overview {
            channels,
            agents,
            runs: self.runs.clone(),
            now,
        }
    }

    fn channel_view(
        &self,
        id: &str,
        before_id: Option<i64>,
        page_messages: usize,
    ) -> Result<ChannelView, SourceError> {
        let c = self
            .channels
            .iter()
            .find(|c| c.channel.id == id)
            .ok_or_else(|| SourceError::NotFound(format!("no channel {id}")))?;
        let earlier: Vec<&Message> = self
            .messages
            .iter()
            .filter(|m| m.channel_id == id && before_id.is_none_or(|b| m.id < b))
            .collect();
        let skip = earlier.len().saturating_sub(page_messages);
        let messages: Vec<Message> = earlier[skip..].iter().map(|m| (*m).clone()).collect();
        let older_before_id = match skip {
            0 => None,
            _ => messages.first().map(|m| m.id),
        };
        Ok(ChannelView {
            channel: c.channel.clone(),
            members: c.members.clone(),
            messages,
            older_before_id,
        })
    }

    fn agent_view(&self, name: &str) -> Result<AgentView, SourceError> {
        let agent = self
            .agents
            .iter()
            .find(|a| a.name == name)
            .cloned()
            .ok_or_else(|| SourceError::NotFound(format!("no agent {name}")))?;
        let channels = self
            .channels
            .iter()
            .filter(|c| c.members.iter().any(|m| m == name))
            .map(|c| c.channel.clone())
            .collect();
        let message_count = self.messages.iter().filter(|m| m.agent == name).count() as i64;
        Ok(AgentView {
            agent,
            channels,
            message_count,
        })
    }
}

/// `messages` counted in stretches of `WEB_ACTIVITY_BUCKET`, oldest first.
fn activity<'a>(messages: impl Iterator<Item = &'a Message>) -> Vec<ActivityCount> {
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for m in messages {
        let start = clock::floor_text(&m.created_at, WEB_ACTIVITY_BUCKET)
            .unwrap_or_else(|| m.created_at.clone());
        *counts.entry(start).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(start, count)| ActivityCount { start, count })
        .collect()
}

/// A message body: usually one line, sometimes several long paragraphs.
fn body(e: &dyn Entropy) -> String {
    if e.below(PREVIEW_LONG_MESSAGE_ONE_IN) == 0 {
        let count = 2 + e.below(6);
        (0..count)
            .map(|_| PARAGRAPHS[e.below(PARAGRAPHS.len())])
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        LINES[e.below(LINES.len())].to_string()
    }
}
