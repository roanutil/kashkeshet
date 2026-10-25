//! The server's side of the web contract: [`RulesSource`] reads through the
//! rules, and [`WebAdmission`] is the gate. Both only translate between the
//! rules' and admission's types and the web's.

use super::source::{
    Admit, Agent, AgentSummary, AgentView, Channel, ChannelLink, ChannelSummary, ChannelView, Gate,
    LiveEvent, Message, Overview, ServerRun, SignIn, Source, SourceError,
};
use crate::admission::{self, Credentials, Decision, WebAdmission};
use crate::events::Notice;
use crate::rules::{self, AgentInfo, ChannelInfo, Code, RuleError, Rules, ServerRunInfo};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;

/// The pages' data, read through the rules: everything, including closed
/// channels, as the human's view always has.
pub struct RulesSource {
    rules: Rules,
}

impl RulesSource {
    pub fn new(rules: Rules) -> RulesSource {
        RulesSource { rules }
    }
}

impl Source for RulesSource {
    fn overview(&self) -> BoxFuture<'_, Result<Overview, SourceError>> {
        Box::pin(async move {
            let o = self.rules.overview().await.map_err(source_error)?;
            Ok(Overview {
                channels: o
                    .channels
                    .into_iter()
                    .map(|c| ChannelSummary {
                        channel: channel(c.info),
                        message_count: c.message_count,
                        last_message_at: c.last_message_at,
                        last_speaker: c.last_speaker,
                        recent_message_times: c.recent_message_times,
                    })
                    .collect(),
                agents: o
                    .agents
                    .into_iter()
                    .map(|a| AgentSummary {
                        agent: agent(a.info),
                        channels: a
                            .channels
                            .into_iter()
                            .map(|c| ChannelLink {
                                id: c.id,
                                name: c.name,
                            })
                            .collect(),
                    })
                    .collect(),
                runs: o.runs.into_iter().map(run).collect(),
                now: o.now,
            })
        })
    }

    fn channel(
        &self,
        id: String,
        before_id: Option<i64>,
    ) -> BoxFuture<'_, Result<ChannelView, SourceError>> {
        Box::pin(async move {
            let v = self
                .rules
                .channel_view(id, before_id)
                .await
                .map_err(source_error)?;
            Ok(ChannelView {
                channel: channel(v.channel),
                members: v.members,
                messages: v.messages.into_iter().map(message).collect(),
                older_before_id: v.older_before_id,
            })
        })
    }

    fn agent(&self, name: String) -> BoxFuture<'_, Result<AgentView, SourceError>> {
        Box::pin(async move {
            let v = self.rules.agent_view(name).await.map_err(source_error)?;
            Ok(AgentView {
                agent: agent(v.agent),
                channels: v.channels.into_iter().map(channel).collect(),
                message_count: v.message_count,
            })
        })
    }

    fn live(&self) -> BoxStream<'static, LiveEvent> {
        self.rules
            .events()
            .subscribe()
            .into_stream()
            .map(|notice| match notice {
                Notice::Message(m) => LiveEvent::Message(Message {
                    id: m.id,
                    channel_id: m.channel_id,
                    channel_name: m.channel_name,
                    agent: m.agent,
                    body: m.body,
                    created_at: m.created_at,
                    run_id: m.run_id,
                }),
                Notice::Missed(n) => LiveEvent::Missed(n),
            })
            .boxed()
    }

    fn page_messages(&self) -> usize {
        self.rules.limits().web_page_messages
    }
}

impl Gate for WebAdmission {
    fn sign_in_required(&self) -> bool {
        WebAdmission::sign_in_required(self)
    }

    fn admits(&self, session_id: Option<&str>) -> Admit {
        match self.decide(session_id) {
            Decision::Admitted(_) => Admit::Yes,
            Decision::Refused(refusal) => Admit::No(refusal.to_string()),
        }
    }

    fn sign_in(&self, key: &str) -> SignIn {
        match WebAdmission::sign_in(self, &Credentials::from_key(key)) {
            admission::SignIn::Started {
                session_id,
                lifetime,
            } => SignIn::Started {
                session_id,
                lifetime,
            },
            admission::SignIn::Refused(refusal) => SignIn::Refused(refusal.to_string()),
        }
    }

    fn sign_out(&self, session_id: &str) {
        WebAdmission::sign_out(self, session_id)
    }
}

fn source_error(e: RuleError) -> SourceError {
    match e.code {
        Code::NotFound => SourceError::NotFound(e.message),
        Code::InvalidArgument => SourceError::Invalid(e.message),
        _ => SourceError::Failed(e.to_string()),
    }
}

fn channel(c: ChannelInfo) -> Channel {
    Channel {
        id: c.id,
        name: c.name,
        purpose: c.purpose,
        open: c.open,
        created_by: c.created_by,
        created_at: c.created_at,
        member_count: c.member_count,
    }
}

fn agent(a: AgentInfo) -> Agent {
    Agent {
        name: a.name,
        created_at: a.created_at,
        last_seen: a.last_seen,
        run_id: a.run_id,
    }
}

fn message(m: rules::Message) -> Message {
    Message {
        id: m.id,
        channel_id: m.channel_id,
        channel_name: m.channel_name,
        agent: m.agent,
        body: m.body,
        created_at: m.created_at,
        run_id: m.run_id,
    }
}

fn run(r: ServerRunInfo) -> ServerRun {
    ServerRun {
        id: r.id,
        started_at: r.started_at,
        version: r.version,
        git_commit: r.git_commit,
        settings: r.settings,
        instructions_sha256: r.instructions_sha256,
    }
}
