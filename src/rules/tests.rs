//! Rule tests against an in-memory database. Each row of the rules table has a test.

use super::*;
use crate::clock::testing::FixedClock;
use crate::events::Notice;
use crate::names::testing::{SameNameEntropy, SeqEntropy};
use chrono::TimeZone;
use futures::FutureExt;
use std::time::Duration;

struct World {
    rules: Rules,
    clock: Arc<FixedClock>,
    run_id: i64,
    /// The operator, on the same database.
    operator: Operator,
}

/// The run every test world records, so stamped rows can be checked.
const TEST_RUN: &str = "test-run";

async fn world_with(limits: Limits, entropy: Arc<dyn Entropy>) -> World {
    let clock = Arc::new(FixedClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 23, 14, 0, 0)
            .unwrap(),
    ));
    let db = Db::open_in_memory().expect("db");
    let run_id = db
        .write(|w| {
            w.insert_server_run(&crate::store::NewServerRunRecord {
                started_at: "2026-10-23T14:00:00Z".into(),
                version: TEST_RUN.into(),
                git_commit: TEST_RUN.into(),
                settings: TEST_RUN.into(),
                instructions_sha256: TEST_RUN.into(),
            })
        })
        .await
        .expect("record run");
    let operator = Operator::new(db.clone(), clock.clone());
    let rules = Rules::new(Deps {
        waiting: crate::rules::Waiting::default(),
        db,
        events: Events::new(),
        clock: clock.clone(),
        entropy,
        sends: SendLimiter::new(limits.sends_per_minute),
        registrations: RegisterLimiter::new(limits.registrations_per_minute),
        run_id,
        limits,
    });
    World {
        rules,
        clock,
        run_id,
        operator,
    }
}

async fn world() -> World {
    world_with(Limits::DEFAULT, Arc::new(SeqEntropy::new(42))).await
}

impl World {
    async fn agent(&self) -> Agent {
        let reg = self.rules.register().await.expect("register");
        self.rules
            .authenticate(Some(&reg.token))
            .await
            .expect("auth")
    }

    async fn channel(&self, owner: &Agent, name: &str, open: bool, members: &[&Agent]) -> String {
        self.rules
            .create_channel(
                owner,
                NewChannel {
                    name: name.into(),
                    open,
                    purpose: None,
                    members: members.iter().map(|a| a.name.clone()).collect(),
                },
            )
            .await
            .expect("create")
            .id
    }

    async fn visible_ids(&self, me: &Agent) -> Vec<String> {
        self.rules
            .list_channels(
                me,
                ChannelQuery {
                    limit: 100,
                    ..Default::default()
                },
            )
            .await
            .expect("list")
            .items
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    async fn read_latest(&self, me: &Agent, ch: &str) -> Result<MessagePage> {
        self.rules
            .read(
                me,
                ReadQuery {
                    channel_id: ch.into(),
                    position: Position::Latest,
                    from_agent: None,
                    limit: 20,
                },
            )
            .await
    }
}

fn code<T: std::fmt::Debug>(r: Result<T>) -> Code {
    r.expect_err("expected an error").code
}

// ---- Identity ----

#[tokio::test]
async fn register_gives_a_name_and_a_working_token() {
    let w = world().await;
    let reg = w.rules.register().await.unwrap();
    assert!(reg.token.starts_with("act_"));
    let me = w.rules.authenticate(Some(&reg.token)).await.unwrap();
    assert_eq!(me.name, reg.name);
}

#[tokio::test]
async fn bad_or_missing_tokens_are_unauthenticated_with_instructions() {
    let w = world().await;
    let err = w.rules.authenticate(None).await.unwrap_err();
    assert_eq!(err.code, Code::Unauthenticated);
    assert!(err.message.contains("register"));
    assert_eq!(
        code(w.rules.authenticate(Some("act_nope")).await),
        Code::Unauthenticated
    );
}

#[tokio::test]
async fn name_clashes_get_a_number() {
    let w = world_with(
        Limits::DEFAULT,
        Arc::new(SameNameEntropy(SeqEntropy::new(3))),
    )
    .await;
    let a = w.rules.register().await.unwrap();
    let b = w.rules.register().await.unwrap();
    let c = w.rules.register().await.unwrap();
    assert_eq!(b.name, format!("{}-2", a.name));
    assert_eq!(c.name, format!("{}-3", a.name));
}

#[tokio::test]
async fn last_seen_records_every_authenticated_call() {
    let w = world().await;
    let reg = w.rules.register().await.unwrap();
    let seen = |w: &World| {
        let rules = w.rules.clone();
        async move {
            let me = Agent {
                name: String::new(),
            };
            rules.list_agents(&me, None, 10, None).await.unwrap().items[0]
                .last_seen
                .clone()
        }
    };
    let first = seen(&w).await;
    w.clock.advance(chrono::Duration::seconds(30));
    w.rules.authenticate(Some(&reg.token)).await.unwrap();
    assert_ne!(seen(&w).await, first);
    w.clock.advance(chrono::Duration::seconds(31));
    w.rules.authenticate(Some(&reg.token)).await.unwrap();
    assert_ne!(seen(&w).await, first);
}

#[tokio::test]
async fn list_agents_pages_by_name() {
    let w = world().await;
    let me = w.agent().await;
    for _ in 0..4 {
        w.agent().await;
    }
    let first = w.rules.list_agents(&me, None, 3, None).await.unwrap();
    assert_eq!(first.items.len(), 3);
    let rest = w
        .rules
        .list_agents(&me, None, 3, first.next_cursor.clone())
        .await
        .unwrap();
    assert_eq!(rest.items.len(), 2);
    assert!(rest.next_cursor.is_none());
    assert!(rest.items[0].name > first.items[2].name);
}

#[tokio::test]
async fn descriptions_are_set_replaced_and_cleared() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[&b]).await;
    let set = w
        .rules
        .set_description(&a, "  Codex, reviews only  ".into())
        .await
        .unwrap();
    assert_eq!(set.description.as_deref(), Some("Codex, reviews only"));
    w.rules
        .set_description(&a, "Claude Code".into())
        .await
        .unwrap();
    let described = |items: &[AgentInfo]| {
        items
            .iter()
            .find(|i| i.name == a.name)
            .and_then(|i| i.description.clone())
    };
    // Others see it in list_agents, list_members and the web page.
    let agents = w.rules.list_agents(&b, None, 10, None).await.unwrap();
    assert_eq!(described(&agents.items).as_deref(), Some("Claude Code"));
    let members = w
        .rules
        .list_members(&b, ch.clone(), 10, None)
        .await
        .unwrap();
    assert_eq!(
        described(&members.page.items).as_deref(),
        Some("Claude Code")
    );
    let view = w.rules.channel_view(ch, None).await.unwrap();
    assert_eq!(described(&view.members).as_deref(), Some("Claude Code"));
    let page = w.rules.agent_view(a.name.clone()).await.unwrap();
    assert_eq!(page.agent.description.as_deref(), Some("Claude Code"));
    // Empty text clears it; it never touches anyone else's.
    let cleared = w.rules.set_description(&a, "   ".into()).await.unwrap();
    assert_eq!(cleared.description, None);
    let agents = w.rules.list_agents(&b, None, 10, None).await.unwrap();
    assert!(agents.items.iter().all(|i| i.description.is_none()));
}

#[tokio::test]
async fn descriptions_over_the_limit_are_refused() {
    let limits = Limits {
        max_description_chars: 5,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(42))).await;
    let a = w.agent().await;
    // The cap counts characters, not bytes.
    let fits = w.rules.set_description(&a, "héllo".into()).await.unwrap();
    assert_eq!(fits.description.as_deref(), Some("héllo"));
    assert_eq!(
        code(w.rules.set_description(&a, "héllo!".into()).await),
        Code::InvalidArgument
    );
    // A refused change keeps the old description.
    let me = w.rules.agent_view(a.name.clone()).await.unwrap();
    assert_eq!(me.agent.description.as_deref(), Some("héllo"));
}

// ---- Rules table: appears in list_channels ----

#[tokio::test]
async fn open_channels_appear_to_everyone() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[]).await;
    assert!(w.visible_ids(&b).await.contains(&ch));
}

#[tokio::test]
async fn closed_channels_appear_to_members_only() {
    let w = world().await;
    let (a, b, c) = (w.agent().await, w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[&b]).await;
    assert!(w.visible_ids(&a).await.contains(&ch));
    assert!(w.visible_ids(&b).await.contains(&ch));
    assert!(!w.visible_ids(&c).await.contains(&ch));
}

// ---- Rules table: join ----

#[tokio::test]
async fn anyone_can_join_an_open_channel() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[]).await;
    let info = w.rules.join_channel(&b, ch.clone()).await.unwrap();
    assert!(info.is_member);
    assert_eq!(info.member_count, 2);
    // Joining again changes nothing.
    let again = w.rules.join_channel(&b, ch).await.unwrap();
    assert_eq!(again.member_count, 2);
}

#[tokio::test]
async fn closed_channels_can_only_be_entered_by_being_added() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[]).await;
    assert_eq!(
        code(w.rules.join_channel(&b, ch.clone()).await),
        Code::NotFound
    );
    w.rules
        .add_to_channel(&a, ch.clone(), vec![b.name.clone()])
        .await
        .unwrap();
    assert!(w.visible_ids(&b).await.contains(&ch));
}

// ---- Rules table: add others ----

#[tokio::test]
async fn any_member_can_add_others_to_an_open_channel() {
    let w = world().await;
    let (a, b, c) = (w.agent().await, w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[]).await;
    // A non-member of an open channel is told to join first.
    assert_eq!(
        code(
            w.rules
                .add_to_channel(&b, ch.clone(), vec![c.name.clone()])
                .await
        ),
        Code::NotMember
    );
    w.rules.join_channel(&b, ch.clone()).await.unwrap();
    let members = w
        .rules
        .add_to_channel(&b, ch, vec![c.name.clone()])
        .await
        .unwrap();
    assert_eq!(members.total, 3);
    assert!(members.page.items.iter().any(|m| m.name == c.name));
}

#[tokio::test]
async fn any_member_can_add_others_to_a_closed_channel() {
    let w = world().await;
    let (a, b, c, d) = (
        w.agent().await,
        w.agent().await,
        w.agent().await,
        w.agent().await,
    );
    let ch = w.channel(&a, "secret", false, &[&b]).await;
    // `b` was added, not the creator, and has the same power.
    w.rules
        .add_to_channel(&b, ch.clone(), vec![c.name.clone()])
        .await
        .unwrap();
    assert!(w.visible_ids(&c).await.contains(&ch));
    assert_eq!(
        code(w.rules.add_to_channel(&d, ch, vec![d.name.clone()]).await),
        Code::NotFound
    );
}

#[tokio::test]
async fn adding_an_unknown_agent_is_not_found() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    assert_eq!(
        code(
            w.rules
                .add_to_channel(&a, ch, vec!["nobody-here".into()])
                .await
        ),
        Code::NotFound
    );
}

// ---- Rules table: read, send, set purpose ----

#[tokio::test]
async fn members_of_an_open_channel_can_read_send_and_set_purpose() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[]).await;

    assert_eq!(
        code(w.rules.send(&b, ch.clone(), "hi".into()).await),
        Code::NotMember
    );
    assert_eq!(code(w.read_latest(&b, &ch).await), Code::NotMember);
    assert_eq!(
        code(w.rules.set_purpose(&b, ch.clone(), "x".into()).await),
        Code::NotMember
    );

    w.rules.join_channel(&b, ch.clone()).await.unwrap();
    w.rules.send(&b, ch.clone(), "hi".into()).await.unwrap();
    assert_eq!(w.read_latest(&b, &ch).await.unwrap().messages.len(), 1);
    let info = w.rules.set_purpose(&b, ch, "talk".into()).await.unwrap();
    assert_eq!(info.purpose.as_deref(), Some("talk"));
}

#[tokio::test]
async fn members_of_a_closed_channel_can_read_send_and_set_purpose() {
    let w = world().await;
    let (a, b, c) = (w.agent().await, w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[&b]).await;

    w.rules.send(&b, ch.clone(), "hi".into()).await.unwrap();
    assert_eq!(w.read_latest(&b, &ch).await.unwrap().messages.len(), 1);
    w.rules
        .set_purpose(&b, ch.clone(), "plans".into())
        .await
        .unwrap();

    assert_eq!(
        code(w.rules.send(&c, ch.clone(), "hi".into()).await),
        Code::NotFound
    );
    assert_eq!(code(w.read_latest(&c, &ch).await), Code::NotFound);
    assert_eq!(
        code(w.rules.set_purpose(&c, ch, "x".into()).await),
        Code::NotFound
    );
}

// ---- The human ----

#[tokio::test]
async fn the_human_changes_any_channel_and_the_operator_says_so() {
    let w = world().await;
    let mut notices = w.rules.events().subscribe();
    let (a, b) = (w.agent().await, w.agent().await);
    // A closed channel the human is not in, as no human is in any.
    let ch = w.channel(&a, "secret", false, &[&b]).await;

    let info = w
        .rules
        .set_purpose(Actor::Human, ch.clone(), "  release prep ".into())
        .await
        .unwrap();
    assert_eq!(info.purpose.as_deref(), Some("release prep"));
    assert!(!info.is_member);

    // The members see the change explained, from the operator.
    let page = w.read_latest(&b, &ch).await.unwrap();
    let said = page.messages.last().expect("an operator message");
    assert_eq!(said.agent, names::OPERATOR);
    assert_eq!(
        said.body,
        "The operator changed the purpose to: release prep"
    );
    assert_eq!(said.run_id, Some(w.run_id));

    w.rules
        .set_purpose(Actor::Human, ch.clone(), " ".into())
        .await
        .unwrap();
    let page = w.read_latest(&a, &ch).await.unwrap();
    assert_eq!(
        page.messages.last().map(|m| m.body.as_str()),
        Some("The operator cleared the purpose.")
    );

    // Waiting agents hear about it like any message.
    let mut heard = Vec::new();
    while let Ok(Some(notice)) =
        tokio::time::timeout(Duration::from_millis(50), notices.next()).await
    {
        if let Notice::Message(m) = notice {
            heard.push((m.channel_id, m.agent));
        }
    }
    assert_eq!(
        heard,
        [
            (ch.clone(), names::OPERATOR.to_string()),
            (ch.clone(), names::OPERATOR.to_string())
        ]
    );
}

#[tokio::test]
async fn the_operator_is_not_an_agent_and_no_agent_can_become_it() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    w.rules
        .set_purpose(Actor::Human, ch.clone(), "talk".into())
        .await
        .unwrap();
    // The operator has spoken, yet is no agent: it cannot be reclaimed,
    // added, or found, and is not listed.
    assert_eq!(
        code(
            w.rules
                .reclaim(names::OPERATOR.into(), "arc_anything".into())
                .await
        ),
        Code::Unauthenticated
    );
    assert_eq!(
        code(
            w.rules
                .add_to_channel(&a, ch.clone(), vec![names::OPERATOR.into()])
                .await
        ),
        Code::NotFound
    );
    let agents = w.rules.list_agents(&a, None, 100, None).await.unwrap();
    assert!(agents.items.iter().all(|i| i.name != names::OPERATOR));
    // Registering, even when every name draw is the same, never gives it.
    let w = world_with(
        Limits::DEFAULT,
        Arc::new(SameNameEntropy(SeqEntropy::new(5))),
    )
    .await;
    for _ in 0..5 {
        let reg = w.rules.register().await.unwrap();
        assert_ne!(reg.name, names::OPERATOR);
        assert!(reg.name.contains('-'), "{}", reg.name);
    }
}

#[tokio::test]
async fn the_human_gets_not_found_for_a_missing_channel_and_bad_input_changes_nothing() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    assert_eq!(
        code(
            w.rules
                .set_purpose(Actor::Human, "ch_missing0".into(), "x".into())
                .await
        ),
        Code::NotFound
    );
    let too_long = "x".repeat(Limits::DEFAULT.max_purpose_chars + 1);
    assert_eq!(
        code(
            w.rules
                .set_purpose(Actor::Human, ch.clone(), too_long)
                .await
        ),
        Code::InvalidArgument
    );
    // Nothing was said: a refused change posts no operator message.
    assert!(w.read_latest(&a, &ch).await.unwrap().messages.is_empty());
}

#[tokio::test]
async fn an_agent_changing_the_purpose_posts_no_operator_message() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    w.rules
        .set_purpose(&a, ch.clone(), "talk".into())
        .await
        .unwrap();
    assert!(w.read_latest(&a, &ch).await.unwrap().messages.is_empty());
}

// ---- Rules table: leave ----

#[tokio::test]
async fn any_member_can_leave_an_open_channel() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[&b]).await;
    w.rules.leave_channel(&b, ch.clone()).await.unwrap();
    assert_eq!(
        code(w.rules.leave_channel(&b, ch.clone()).await),
        Code::NotMember
    );
    // Still visible, since it is open.
    assert!(w.visible_ids(&b).await.contains(&ch));
}

#[tokio::test]
async fn any_member_can_leave_a_closed_channel_and_it_then_vanishes_for_them() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[&b]).await;
    w.rules.leave_channel(&b, ch.clone()).await.unwrap();
    assert!(!w.visible_ids(&b).await.contains(&ch));
    assert_eq!(code(w.rules.leave_channel(&b, ch).await), Code::NotFound);
}

#[tokio::test]
async fn a_closed_channel_with_no_members_is_unreachable_but_the_human_sees_it() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[]).await;
    w.rules
        .send(&a, ch.clone(), "last words".into())
        .await
        .unwrap();
    w.rules.leave_channel(&a, ch.clone()).await.unwrap();
    for who in [&a, &b] {
        assert_eq!(
            code(w.rules.join_channel(who, ch.clone()).await),
            Code::NotFound
        );
    }
    let view = w.rules.channel_view(ch, None).await.unwrap();
    assert_eq!(view.messages.len(), 1);
    assert!(view.members.is_empty());
}

#[tokio::test]
async fn the_web_channel_page_holds_web_page_messages_and_pages_back() {
    let limits = Limits {
        web_page_messages: 3,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(42))).await;
    let a = w.agent().await;
    let ch = w.channel(&a, "busy", true, &[]).await;
    let mut ids = Vec::new();
    for i in 0..7 {
        let m = w.rules.send(&a, ch.clone(), format!("m{i}")).await.unwrap();
        ids.push(m.id);
    }
    let shown = |v: &human::ChannelView| v.messages.iter().map(|m| m.id).collect::<Vec<_>>();

    let latest = w.rules.channel_view(ch.clone(), None).await.unwrap();
    assert_eq!(shown(&latest), ids[4..7]);
    assert_eq!(latest.older_before_id, Some(ids[4]));

    let older = w
        .rules
        .channel_view(ch.clone(), Some(ids[4]))
        .await
        .unwrap();
    assert_eq!(shown(&older), ids[1..4]);
    assert_eq!(older.older_before_id, Some(ids[1]));

    let oldest = w.rules.channel_view(ch, Some(ids[1])).await.unwrap();
    assert_eq!(shown(&oldest), ids[0..1]);
    assert_eq!(oldest.older_before_id, None);
}

#[tokio::test]
async fn the_front_page_summarizes_each_channel_and_agent() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let busy = w.channel(&a, "busy", true, &[&b]).await;
    let quiet = w.channel(&b, "quiet", false, &[]).await;
    w.rules.send(&a, busy.clone(), "one".into()).await.unwrap();
    w.clock.advance(chrono::Duration::minutes(10));
    w.rules.send(&b, busy.clone(), "two".into()).await.unwrap();
    w.rules
        .send(&a, busy.clone(), "three".into())
        .await
        .unwrap();

    let o = w.rules.overview().await.unwrap();
    assert_eq!(o.now, clock::to_text(w.clock.now()));
    let row = |id: &str| o.channels.iter().find(|c| c.info.id == id).unwrap();
    let busy_row = row(&busy);
    assert_eq!(busy_row.message_count, 3);
    assert_eq!(busy_row.last_speaker.as_deref(), Some(a.name.as_str()));
    assert_eq!(
        busy_row.last_message_at,
        Some(clock::to_text(w.clock.now()))
    );
    // Only the two messages from the last five minutes are recent.
    let recent =
        |c: &human::ChannelActivity| c.recent_activity.iter().map(|a| a.count).sum::<i64>();
    assert_eq!(recent(busy_row), 2);
    let quiet_row = row(&quiet);
    assert_eq!(quiet_row.message_count, 0);
    assert_eq!(quiet_row.last_speaker, None);
    assert!(quiet_row.recent_activity.is_empty());

    let channels_of = |name: &str| {
        let mut ids: Vec<String> = o
            .agents
            .iter()
            .find(|x| x.info.name == name)
            .unwrap()
            .channels
            .iter()
            .map(|c| c.id.clone())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(channels_of(&a.name), std::slice::from_ref(&busy));
    let mut both = vec![busy.clone(), quiet.clone()];
    both.sort();
    assert_eq!(channels_of(&b.name), both);

    // Later, nothing is recent.
    w.clock.advance(chrono::Duration::minutes(6));
    let later = w.rules.overview().await.unwrap();
    assert!(later.channels.iter().all(|c| c.recent_activity.is_empty()));
}

/// A busy channel does not crowd a quiet one's recent count out.
#[tokio::test]
async fn recent_counts_are_exact_for_every_channel() {
    let limits = Limits {
        sends_per_minute: 100_000,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(42))).await;
    let a = w.agent().await;
    let quiet = w.channel(&a, "quiet", true, &[]).await;
    let busy = w.channel(&a, "busy", true, &[]).await;
    w.rules
        .send(&a, quiet.clone(), "hello".into())
        .await
        .unwrap();
    for i in 0..1005 {
        w.rules
            .send(&a, busy.clone(), format!("m{i}"))
            .await
            .unwrap();
    }
    let o = w.rules.overview().await.unwrap();
    let recent = |id: &str| {
        o.channels
            .iter()
            .find(|c| c.info.id == id)
            .unwrap()
            .recent_activity
            .iter()
            .map(|a| a.count)
            .sum::<i64>()
    };
    assert_eq!(recent(&quiet), 1);
    assert_eq!(recent(&busy), 1005);
}

/// A message stored later but stamped earlier (the clock moved back) does
/// not hide the recent ones.
#[tokio::test]
async fn recent_counts_do_not_assume_times_follow_message_order() {
    let limits = Limits {
        sends_per_minute: 100_000,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(42))).await;
    let a = w.agent().await;
    let busy = w.channel(&a, "busy", true, &[]).await;
    for i in 0..1001 {
        w.rules
            .send(&a, busy.clone(), format!("m{i}"))
            .await
            .unwrap();
    }
    w.clock.advance(chrono::Duration::hours(-1));
    w.rules
        .send(&a, busy.clone(), "from before".into())
        .await
        .unwrap();
    w.clock.advance(chrono::Duration::hours(1));
    let o = w.rules.overview().await.unwrap();
    let row = o.channels.iter().find(|c| c.info.id == busy).unwrap();
    assert_eq!(
        row.recent_activity.iter().map(|a| a.count).sum::<i64>(),
        1001
    );
    assert_eq!(row.message_count, 1002);
}

// ---- Closed channels do not exist to outsiders ----

#[tokio::test]
async fn hidden_and_missing_channels_look_the_same() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[]).await;
    let msg = w.rules.send(&a, ch.clone(), "psst".into()).await.unwrap();
    let missing = "ch_zzzzzzzz".to_string();

    for id in [ch.clone(), missing.clone()] {
        let hidden = w
            .rules
            .list_members(&b, id.clone(), 10, None)
            .await
            .unwrap_err();
        assert_eq!(hidden.code, Code::NotFound);
        assert_eq!(hidden.message, format!("no channel {id}"));
        assert_eq!(
            code(w.rules.join_channel(&b, id.clone()).await),
            Code::NotFound
        );
        assert_eq!(
            code(w.rules.leave_channel(&b, id.clone()).await),
            Code::NotFound
        );
        assert_eq!(
            code(w.rules.send(&b, id.clone(), "x".into()).await),
            Code::NotFound
        );
        assert_eq!(code(w.read_latest(&b, &id).await), Code::NotFound);
        assert_eq!(
            code(
                w.rules
                    .add_to_channel(&b, id.clone(), vec![b.name.clone()])
                    .await
            ),
            Code::NotFound
        );
        assert_eq!(
            code(w.rules.set_purpose(&b, id.clone(), "x".into()).await),
            Code::NotFound
        );
        let search = SearchQuery {
            query: "psst".into(),
            channel_id: Some(id.clone()),
            from_agent: None,
            limit: 10,
        };
        assert_eq!(code(w.rules.search(&b, search).await), Code::NotFound);
        let wait = WaitQuery {
            after_id: 0,
            channel_ids: Some(vec![id.clone()]),
            timeout: Duration::from_millis(1),
            limit: 10,
        };
        assert_eq!(
            code(w.rules.wait_for_messages(&b, wait).await),
            Code::NotFound
        );
    }
    let hidden_msg = w.rules.get_message(&b, msg.id).await.unwrap_err();
    let missing_msg = w.rules.get_message(&b, 999_999).await.unwrap_err();
    assert_eq!(hidden_msg.code, Code::NotFound);
    assert_eq!(missing_msg.code, Code::NotFound);
}

#[tokio::test]
async fn channel_names_can_repeat() {
    let w = world().await;
    let a = w.agent().await;
    let one = w.channel(&a, "general", true, &[]).await;
    let two = w.channel(&a, "general", false, &[]).await;
    assert_ne!(one, two);
}

#[tokio::test]
async fn members_can_read_history_from_before_they_joined() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[]).await;
    w.rules.send(&a, ch.clone(), "before".into()).await.unwrap();
    w.rules
        .add_to_channel(&a, ch.clone(), vec![b.name.clone()])
        .await
        .unwrap();
    let page = w.read_latest(&b, &ch).await.unwrap();
    assert_eq!(page.messages[0].body, "before");
}

// ---- list_channels filters ----

#[tokio::test]
async fn list_channels_filters_and_pages() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let parser = w.channel(&a, "parser", true, &[]).await;
    w.rules
        .set_purpose(&a, parser.clone(), "work on the TOML parser".into())
        .await
        .unwrap();
    let shared = w.channel(&a, "pair", false, &[&b]).await;
    let mine = w.channel(&b, "notes", true, &[]).await;

    let found = w
        .rules
        .list_channels(
            &b,
            ChannelQuery {
                query: Some("toml".into()),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        found.items.iter().map(|c| &c.id).collect::<Vec<_>>(),
        vec![&parser]
    );

    let member_only = w
        .rules
        .list_channels(
            &b,
            ChannelQuery {
                member_only: true,
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ids: Vec<_> = member_only.items.iter().map(|c| c.id.clone()).collect();
    assert!(ids.contains(&shared) && ids.contains(&mine) && !ids.contains(&parser));

    let with = w
        .rules
        .list_channels(
            &b,
            ChannelQuery {
                with_agent: Some(a.name.clone()),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        with.items.iter().map(|c| &c.id).collect::<Vec<_>>(),
        vec![&shared]
    );

    // Paging one at a time visits every visible channel once.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = w
            .rules
            .list_channels(
                &b,
                ChannelQuery {
                    limit: 1,
                    cursor,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        seen.extend(page.items.into_iter().map(|c| c.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 3);
}

#[tokio::test]
async fn list_channels_counts_new_messages() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    let first = w.rules.send(&a, ch.clone(), "one".into()).await.unwrap();
    w.rules.send(&a, ch.clone(), "two".into()).await.unwrap();
    let page = w
        .rules
        .list_channels(
            &a,
            ChannelQuery {
                after_id: Some(first.id),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items[0].new_messages, Some(1));
}

// ---- Messages ----

#[tokio::test]
async fn send_checks_body() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    assert_eq!(
        code(w.rules.send(&a, ch.clone(), "x".repeat(8 * 1024 + 1)).await),
        Code::BodyTooLong
    );
    let reg = w.rules.register().await.unwrap();
    assert_eq!(
        code(
            w.rules
                .send(&a, ch.clone(), format!("token: {}", reg.token))
                .await
        ),
        Code::ContainsSecret
    );
    assert_eq!(
        code(w.rules.send(&a, ch, "  ".into()).await),
        Code::InvalidArgument
    );
}

#[tokio::test]
async fn send_is_rate_limited() {
    let limits = Limits {
        sends_per_minute: 2,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(1))).await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    w.rules.send(&a, ch.clone(), "1".into()).await.unwrap();
    w.rules.send(&a, ch.clone(), "2".into()).await.unwrap();
    assert_eq!(
        code(w.rules.send(&a, ch.clone(), "3".into()).await),
        Code::RateLimited
    );
    w.clock.advance(chrono::Duration::seconds(30));
    w.rules.send(&a, ch, "4".into()).await.unwrap();
}

#[tokio::test]
async fn a_failed_send_does_not_use_up_the_allowance() {
    let limits = Limits {
        sends_per_minute: 1,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(1))).await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    let sql = |sql: &'static str| w.rules.db().execute_for_tests(sql);
    sql("CREATE TRIGGER fail BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT, 'boom'); END")
        .await
        .unwrap();
    assert_eq!(
        code(w.rules.send(&a, ch.clone(), "lost".into()).await),
        Code::Internal
    );
    sql("DROP TRIGGER fail").await.unwrap();
    w.rules.send(&a, ch.clone(), "kept".into()).await.unwrap();
    // The allowance is one a minute, so the refunded send was the only one.
    assert_eq!(
        code(w.rules.send(&a, ch, "too many".into()).await),
        Code::RateLimited
    );
}

#[tokio::test]
async fn read_pages_both_ways() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    let other = w.channel(&a, "other", true, &[]).await;
    let mut ids = Vec::new();
    for i in 0..5 {
        ids.push(
            w.rules
                .send(&a, ch.clone(), format!("m{i}"))
                .await
                .unwrap()
                .id,
        );
        // Interleave messages in another channel so IDs have gaps.
        w.rules
            .send(&a, other.clone(), "noise".into())
            .await
            .unwrap();
    }
    let q = |position| ReadQuery {
        channel_id: ch.clone(),
        position,
        from_agent: None,
        limit: 2,
    };
    let latest = w.rules.read(&a, q(Position::Latest)).await.unwrap();
    assert_eq!(
        latest.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        ids[3..5]
    );
    assert_eq!(latest.more, Some(Position::Before(ids[3])));

    let older = w.rules.read(&a, q(Position::Before(ids[3]))).await.unwrap();
    assert_eq!(
        older.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        ids[1..3]
    );

    let after = w.rules.read(&a, q(Position::After(ids[0]))).await.unwrap();
    assert_eq!(
        after.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        ids[1..3]
    );
    assert_eq!(after.more, Some(Position::After(ids[2])));

    let end = w.rules.read(&a, q(Position::After(ids[3]))).await.unwrap();
    assert_eq!(end.messages.len(), 1);
    assert_eq!(end.more, None);
}

#[tokio::test]
async fn read_filters_by_agent() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[&b]).await;
    w.rules.send(&a, ch.clone(), "from a".into()).await.unwrap();
    w.rules.send(&b, ch.clone(), "from b".into()).await.unwrap();
    let page = w
        .rules
        .read(
            &a,
            ReadQuery {
                channel_id: ch,
                position: Position::Latest,
                from_agent: Some(b.name.clone()),
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].agent, b.name);
}

#[tokio::test]
async fn get_message_needs_membership() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[]).await;
    let msg = w.rules.send(&a, ch, "hello".into()).await.unwrap();
    assert_eq!(w.rules.get_message(&a, msg.id).await.unwrap(), msg);
    assert_eq!(code(w.rules.get_message(&b, msg.id).await), Code::NotMember);
}

#[tokio::test]
async fn search_covers_only_my_channels() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let mine = w.channel(&a, "mine", true, &[]).await;
    let theirs = w.channel(&b, "theirs", true, &[]).await;
    w.rules
        .send(&a, mine.clone(), "the parser is broken".into())
        .await
        .unwrap();
    w.rules
        .send(&b, theirs, "parser works for me".into())
        .await
        .unwrap();
    let hits = w
        .rules
        .search(
            &a,
            SearchQuery {
                query: "parser".into(),
                channel_id: None,
                from_agent: None,
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].channel_id, mine);
    assert!(hits[0].excerpt.contains("**parser**"));
}

#[tokio::test]
async fn activity_reports_channels_with_newer_messages() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let one = w.channel(&a, "one", true, &[&b]).await;
    let two = w.channel(&a, "two", true, &[&b]).await;
    let start = w.rules.send(&a, one.clone(), "x".into()).await.unwrap().id;
    w.rules.send(&a, one.clone(), "y".into()).await.unwrap();
    w.rules.send(&a, two.clone(), "z".into()).await.unwrap();
    w.rules.send(&a, two.clone(), "w".into()).await.unwrap();
    let rows = w.rules.activity(&b, start).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].channel_id, two);
    assert_eq!(rows[0].new_messages, 2);
    assert_eq!(rows[1].new_messages, 1);
}

#[tokio::test]
async fn wait_returns_at_once_when_messages_exist() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    for i in 0..3 {
        w.rules.send(&a, ch.clone(), format!("m{i}")).await.unwrap();
    }
    let got = w
        .rules
        .wait_for_messages(
            &a,
            WaitQuery {
                after_id: 0,
                channel_ids: None,
                timeout: Duration::from_secs(60),
                limit: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(got.messages.len(), 2);
    assert_eq!(got.more_waiting, 1);
}

#[tokio::test]
async fn wait_wakes_when_a_message_arrives() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[&b]).await;
    let waiter = {
        let rules = w.rules.clone();
        let b = b.clone();
        tokio::spawn(async move {
            rules
                .wait_for_messages(
                    &b,
                    WaitQuery {
                        after_id: 0,
                        channel_ids: None,
                        timeout: Duration::from_secs(10),
                        limit: 10,
                    },
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    w.rules.send(&a, ch, "ping".into()).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("waiter should wake")
        .expect("join")
        .expect("wait");
    assert_eq!(got.messages[0].body, "ping");
}

/// Waits (in real time, briefly) until a wait has started sleeping on the test clock.
async fn until_sleeping(clock: &FixedClock) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while clock.sleepers() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the wait should start sleeping");
}

#[tokio::test]
async fn wait_ignores_channels_i_am_not_in_and_times_out() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[]).await;
    let waiter = {
        let rules = w.rules.clone();
        let b = b.clone();
        tokio::spawn(async move {
            rules
                .wait_for_messages(
                    &b,
                    WaitQuery {
                        after_id: 0,
                        channel_ids: None,
                        timeout: Duration::from_secs(30),
                        limit: 10,
                    },
                )
                .await
        })
    };
    until_sleeping(&w.clock).await;
    w.rules.send(&a, ch, "not for b".into()).await.unwrap();
    w.clock.advance(chrono::Duration::seconds(30));
    let got = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the wait should time out")
        .expect("join")
        .expect("wait");
    assert!(got.messages.is_empty());
}

#[tokio::test]
async fn new_agents_channels_joins_and_leaves_are_published() {
    let w = world().await;
    let mut notices = w.rules.events().subscribe();
    let a = w.agent().await;
    let b = w.agent().await;
    let c = w.agent().await;
    let open = w.channel(&a, "open-room", true, &[]).await;
    let closed = w.channel(&a, "closed-room", false, &[]).await;
    w.rules.join_channel(&b, open.clone()).await.unwrap();
    // Joining again, or being in a channel from its creation, is not news.
    w.rules.join_channel(&b, open.clone()).await.unwrap();
    w.rules.join_channel(&a, open.clone()).await.unwrap();
    // Leaving and coming back: each event carries the count afterwards.
    w.rules.leave_channel(&b, open.clone()).await.unwrap();
    w.rules.join_channel(&b, open.clone()).await.unwrap();
    // Being added by a member is joining; adding a member again is not.
    w.rules
        .add_to_channel(&a, open.clone(), vec![c.name.clone(), b.name.clone()])
        .await
        .unwrap();

    let mut heard = Vec::new();
    while let Ok(Some(notice)) =
        tokio::time::timeout(Duration::from_millis(50), notices.next()).await
    {
        heard.push(match notice {
            Notice::Agent(x) => format!("agent {}", x.name),
            Notice::Channel(x) => format!(
                "channel {} {} {} {} {}",
                x.id, x.name, x.open, x.created_by, x.member_count
            ),
            Notice::Joined(x) => format!(
                "joined {} {} {} {}",
                x.agent, x.channel_id, x.channel_name, x.member_count
            ),
            Notice::Left(x) => format!(
                "left {} {} {} {}",
                x.agent, x.channel_id, x.channel_name, x.member_count
            ),
            other => format!("{other:?}"),
        });
    }
    assert_eq!(
        heard,
        [
            format!("agent {}", a.name),
            format!("agent {}", b.name),
            format!("agent {}", c.name),
            format!("channel {open} open-room true {} 1", a.name),
            format!("channel {closed} closed-room false {} 1", a.name),
            format!("joined {} {open} open-room 2", b.name),
            format!("left {} {open} open-room 1", b.name),
            format!("joined {} {open} open-room 2", b.name),
            format!("joined {} {open} open-room 3", c.name),
        ]
    );
}

#[tokio::test]
async fn a_waiting_agent_is_not_woken_by_new_agents_or_channels() {
    let w = world().await;
    let a = w.agent().await;
    let waiter = {
        let rules = w.rules.clone();
        let a = a.clone();
        tokio::spawn(async move {
            rules
                .wait_for_messages(
                    &a,
                    WaitQuery {
                        after_id: 0,
                        channel_ids: None,
                        timeout: Duration::from_secs(30),
                        limit: 10,
                    },
                )
                .await
        })
    };
    until_sleeping(&w.clock).await;
    let b = w.agent().await;
    let ch = w.channel(&b, "elsewhere", true, &[]).await;
    w.rules.join_channel(&a, ch).await.unwrap();
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished(), "the wait ended without a message");
    w.clock.advance(chrono::Duration::seconds(30));
    let got = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the wait should time out")
        .expect("join")
        .expect("wait");
    assert!(got.messages.is_empty());
}

#[tokio::test]
async fn wait_times_out_by_the_injected_clock() {
    let w = world().await;
    let b = w.agent().await;
    let waiter = {
        let rules = w.rules.clone();
        tokio::spawn(async move {
            rules
                .wait_for_messages(
                    &b,
                    WaitQuery {
                        after_id: 0,
                        channel_ids: None,
                        timeout: Duration::from_secs(30),
                        limit: 10,
                    },
                )
                .await
        })
    };
    until_sleeping(&w.clock).await;
    w.clock.advance(chrono::Duration::seconds(29));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiter.is_finished(), "the wait ended before its deadline");
    w.clock.advance(chrono::Duration::seconds(1));
    let got = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("the wait should time out")
        .expect("join")
        .expect("wait");
    assert!(got.messages.is_empty());
}

#[tokio::test]
async fn create_channel_with_members_and_unknown_member() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let info = w
        .rules
        .create_channel(
            &a,
            NewChannel {
                name: "dm".into(),
                open: false,
                purpose: Some("just us".into()),
                members: vec![b.name.clone(), a.name.clone()],
            },
        )
        .await
        .unwrap();
    assert_eq!(info.member_count, 2);
    assert!(!info.open);
    assert!(info.id.starts_with("ch_"));
    let bad = w
        .rules
        .create_channel(
            &a,
            NewChannel {
                name: "x".into(),
                open: true,
                purpose: None,
                members: vec!["ghost".into()],
            },
        )
        .await;
    assert_eq!(code(bad), Code::NotFound);
}

// ---- Provenance ----

#[tokio::test]
async fn new_agents_and_messages_record_the_server_run() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    let sent = w.rules.send(&a, ch, "hello".into()).await.unwrap();
    assert_eq!(sent.run_id, Some(w.run_id));

    let overview = w.rules.overview().await.unwrap();
    assert_eq!(overview.agents[0].info.run_id, Some(w.run_id));
    let runs: Vec<(i64, &str)> = overview
        .runs
        .iter()
        .map(|r| (r.id, r.version.as_str()))
        .collect();
    assert_eq!(runs, [(w.run_id, TEST_RUN)]);
}

// ---- registration caps and revocation ----

#[tokio::test]
async fn registration_is_rate_limited_across_the_server() {
    let limits = Limits {
        registrations_per_minute: 2,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(42))).await;
    w.rules.register().await.expect("first");
    w.rules.register().await.expect("second");
    let err = w.rules.register().await.expect_err("third");
    assert_eq!(err.code, Code::RateLimited);
    w.clock.advance(chrono::Duration::seconds(30));
    w.rules.register().await.expect("refilled");
}

#[tokio::test]
async fn registration_stops_at_the_agent_cap_and_revoking_frees_room() {
    let limits = Limits {
        max_agents: 2,
        registrations_per_minute: 3,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(42))).await;
    let a = w.agent().await;
    w.agent().await;
    let err = w.rules.register().await.expect_err("over the cap");
    assert_eq!(err.code, Code::LimitReached);
    w.operator.revoke(a.name.clone()).await.expect("revoke");
    // The refused registration gave back its use of the rate limit, so this
    // is the third of three.
    w.rules.register().await.expect("room after revoking");
}

#[tokio::test]
async fn a_revoked_token_is_refused_like_an_unknown_one() {
    let w = world().await;
    let reg = w.rules.register().await.expect("register");
    let me = w.rules.authenticate(Some(&reg.token)).await.expect("auth");
    let other = w.agent().await;
    let ch = w.channel(&me, "general", true, &[&other]).await;
    w.rules
        .send(&me, ch.clone(), "before".into())
        .await
        .expect("send");

    let revoked = w.operator.revoke(me.name.clone()).await.expect("revoke");
    assert_eq!(
        revoked,
        Revoked::Now {
            at: "2026-10-23T14:00:00Z".into()
        }
    );
    let refused = w
        .rules
        .authenticate(Some(&reg.token))
        .await
        .expect_err("revoked");
    let unknown = w
        .rules
        .authenticate(Some("act_00000000000000000000000000"))
        .await
        .expect_err("unknown");
    assert_eq!(refused, unknown);

    // The agent's messages and membership stay for everyone else.
    let page = w
        .rules
        .read(
            &other,
            ReadQuery {
                channel_id: ch.clone(),
                position: Position::Latest,
                from_agent: None,
                limit: 10,
            },
        )
        .await
        .expect("read");
    assert_eq!(page.messages[0].agent, me.name);
}

#[tokio::test]
async fn revoking_twice_or_an_unknown_agent_is_reported() {
    let w = world().await;
    let a = w.agent().await;
    w.operator.revoke(a.name.clone()).await.expect("revoke");
    w.clock.advance(chrono::Duration::minutes(5));
    assert_eq!(
        w.operator.revoke(a.name.clone()).await.expect("again"),
        Revoked::Already {
            at: "2026-10-23T14:00:00Z".into()
        }
    );
    let err = w
        .operator
        .revoke("no-such-agent".into())
        .await
        .expect_err("unknown");
    assert_eq!(err.code, Code::NotFound);
}

#[tokio::test]
async fn the_operator_lists_agents_with_their_revocations() {
    let w = world().await;
    let a = w.agent().await;
    w.clock.advance(chrono::Duration::minutes(5));
    let b = w.agent().await;
    w.operator.revoke(a.name.clone()).await.expect("revoke");
    let agents = w.operator.agents().await.expect("list");
    let names: Vec<_> = agents.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, [b.name.as_str(), a.name.as_str()]);
    assert_eq!(agents[0].revoked_at, None);
    assert_eq!(
        agents[1].revoked_at.as_deref(),
        Some("2026-10-23T14:05:00Z")
    );
}

#[tokio::test]
async fn reclaim_preserves_identity_history_membership_and_provenance() {
    let w = world().await;
    let reg = w.rules.register().await.unwrap();
    let me = w.rules.authenticate(Some(&reg.token)).await.unwrap();
    let channel = w.channel(&me, "private", false, &[]).await;
    let sent = w
        .rules
        .send(&me, channel.clone(), "before restart".into())
        .await
        .unwrap();
    let name = reg.name.clone();
    let before = w
        .rules
        .db()
        .read(move |r| r.agent(&name))
        .await
        .unwrap()
        .unwrap();
    w.clock.advance(chrono::Duration::seconds(75));
    let recovered = w
        .rules
        .reclaim(reg.name.clone(), reg.recovery_code.clone())
        .await
        .unwrap();
    assert_eq!(recovered.name, reg.name);
    assert_ne!(recovered.token, reg.token);
    assert_eq!(
        code(w.rules.authenticate(Some(&reg.token)).await),
        Code::Unauthenticated
    );
    let me = w.rules.authenticate(Some(&recovered.token)).await.unwrap();
    assert_eq!(w.visible_ids(&me).await, [channel]);
    assert_eq!(
        w.rules.get_message(&me, sent.id).await.unwrap().body,
        "before restart"
    );
    let name = reg.name.clone();
    let after = w
        .rules
        .db()
        .read(move |r| r.agent(&name))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.run_id, before.run_id);
    assert_eq!(after.last_seen, Some(w.rules.now_text()));
    // The code still works after a successful recovery (including a lost response).
    let again = w.rules.reclaim(reg.name, reg.recovery_code).await.unwrap();
    assert_ne!(again.token, recovered.token);
    assert_eq!(
        code(w.rules.authenticate(Some(&recovered.token)).await),
        Code::Unauthenticated
    );
    w.rules.authenticate(Some(&again.token)).await.unwrap();
}

#[tokio::test]
async fn reclaim_rejects_wrong_credentials_and_revoked_identities_identically() {
    let w = world().await;
    let a = w.rules.register().await.unwrap();
    let b = w.rules.register().await.unwrap();
    let mut errors = vec![];
    for (name, recovery) in [
        (a.name.clone(), b.recovery_code.clone()),
        ("missing-agent".into(), a.recovery_code.clone()),
        (a.name.clone(), String::new()),
        (a.name.clone(), a.token.clone()),
    ] {
        errors.push(w.rules.reclaim(name, recovery).await.unwrap_err());
    }
    // Failed attempts leave the current token usable.
    w.rules.authenticate(Some(&a.token)).await.unwrap();
    w.operator.revoke(a.name.clone()).await.unwrap();
    errors.push(w.rules.reclaim(a.name, a.recovery_code).await.unwrap_err());
    for error in errors {
        assert_eq!(error.code, Code::Unauthenticated);
        assert_eq!(
            error.message,
            "unknown name or recovery code, or identity unavailable"
        );
    }
}

#[tokio::test]
async fn recovery_codes_are_hashed_and_rejected_in_messages() {
    let w = world().await;
    let reg = w.rules.register().await.unwrap();
    let me = w.rules.authenticate(Some(&reg.token)).await.unwrap();
    let channel = w.channel(&me, "general", true, &[]).await;
    let name = reg.name.clone();
    let recovery = reg.recovery_code.clone();
    assert!(
        w.rules
            .db()
            .read(move |r| r.agent_by_recovery_hash(&name, recovery.as_bytes()))
            .await
            .unwrap()
            .is_none()
    );
    let name = reg.name.clone();
    let hash = auth::hash_token(&reg.recovery_code);
    assert!(
        w.rules
            .db()
            .read(move |r| r.agent_by_recovery_hash(&name, &hash))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        code(w.rules.authenticate(Some(&reg.recovery_code)).await),
        Code::Unauthenticated
    );
    let err = w
        .rules
        .send(&me, channel, format!("recovery: {}", reg.recovery_code))
        .await
        .unwrap_err();
    assert_eq!(err.code, Code::ContainsSecret);
    assert!(!err.message.contains(&reg.recovery_code));
}

#[tokio::test]
async fn legacy_agents_keep_their_token_but_have_no_recovery_code() {
    let w = world().await;
    let run_id = w.run_id;
    w.rules
        .db()
        .write(move |store| {
            store.insert_agent(&NewAgentRecord {
                name: "legacy-agent".into(),
                token_hash: auth::hash_token("old-token"),
                recovery_hash: None,
                created_at: "old-time".into(),
                last_seen: None,
                run_id,
            })
        })
        .await
        .unwrap();
    assert_eq!(
        w.rules.authenticate(Some("old-token")).await.unwrap().name,
        "legacy-agent"
    );
    let reg = w.rules.register().await.unwrap();
    assert_eq!(
        code(
            w.rules
                .reclaim("legacy-agent".into(), reg.recovery_code)
                .await
        ),
        Code::Unauthenticated
    );
    assert_eq!(
        code(w.rules.reclaim("legacy-agent".into(), String::new()).await),
        Code::Unauthenticated
    );
    w.rules.authenticate(Some("old-token")).await.unwrap();
}

#[tokio::test]
async fn channel_notes_are_replaced_cleared_and_attributed_to_the_last_editor() {
    let w = world_with(
        Limits {
            max_note_chars: 4,
            ..Limits::DEFAULT
        },
        Arc::new(SeqEntropy::new(42)),
    )
    .await;
    let a = w.agent().await;
    let b = w.agent().await;
    let ch = w.channel(&a, "notes", true, &[&b]).await;
    let empty = w.rules.get_note(&a, ch.clone()).await.unwrap();
    assert_eq!(empty.text, "");
    assert_eq!(empty.edited_by, None);
    assert_eq!(empty.edited_at, None);
    w.rules
        .set_note(&a, ch.clone(), " 😀\n\t".into())
        .await
        .unwrap();
    let first = w.rules.get_note(&b, ch.clone()).await.unwrap();
    assert_eq!(first.text, " 😀\n\t");
    assert_eq!(first.edited_by.as_deref(), Some(a.name.as_str()));
    assert_eq!(first.edited_at, Some(clock::to_text(w.clock.now())));
    assert_eq!(
        code(w.rules.set_note(&b, ch.clone(), "😀".repeat(5)).await),
        Code::InvalidArgument
    );
    assert_eq!(
        w.rules.get_note(&b, ch.clone()).await.unwrap().text,
        first.text
    );
    w.clock.advance(chrono::Duration::seconds(10));
    w.rules
        .set_note(&b, ch.clone(), "next".into())
        .await
        .unwrap();
    let changed = w.rules.get_note(&a, ch.clone()).await.unwrap();
    assert_eq!(changed.text, "next");
    assert_eq!(changed.edited_by.as_deref(), Some(b.name.as_str()));
    assert_eq!(changed.edited_at, Some(clock::to_text(w.clock.now())));
    let listed = w
        .rules
        .list_channels(
            &a,
            ChannelQuery {
                limit: 100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.items[0].note.text, "next");
    assert_eq!(
        w.rules
            .channel_view(ch.clone(), None)
            .await
            .unwrap()
            .channel
            .note
            .text,
        "next"
    );
    w.rules
        .set_note(&a, ch.clone(), String::new())
        .await
        .unwrap();
    let cleared = w.rules.get_note(&b, ch).await.unwrap();
    assert!(cleared.text.is_empty());
    assert_eq!(cleared.edited_by.as_deref(), Some(a.name.as_str()));
    assert!(cleared.edited_at.is_some());
}

#[tokio::test]
async fn channel_notes_use_channel_visibility_and_membership_rules() {
    let w = world().await;
    let a = w.agent().await;
    let b = w.agent().await;
    let c = w.agent().await;
    for open in [true, false] {
        let ch = w.channel(&a, "notes", open, &[&b]).await;
        w.rules
            .set_note(&b, ch.clone(), "member note".into())
            .await
            .unwrap();
        assert_eq!(
            w.rules.get_note(&a, ch.clone()).await.unwrap().text,
            "member note"
        );
        let expected = if open {
            Code::NotMember
        } else {
            Code::NotFound
        };
        assert_eq!(
            code(w.rules.set_note(&c, ch.clone(), "outsider".into()).await),
            expected
        );
        // Permission checks precede length validation, preserving closed-channel privacy.
        assert_eq!(
            code(w.rules.set_note(&c, ch.clone(), "x".repeat(10001)).await),
            expected
        );
        if open {
            assert_eq!(w.rules.get_note(&c, ch).await.unwrap().text, "member note");
        } else {
            assert_eq!(code(w.rules.get_note(&c, ch).await), Code::NotFound);
        }
    }
    assert_eq!(
        code(w.rules.get_note(&a, "missing".into()).await),
        Code::NotFound
    );
    assert_eq!(
        code(w.rules.set_note(&a, "missing".into(), "x".into()).await),
        Code::NotFound
    );
}

async fn observed(w: &World, viewer: &Agent, name: &str) -> AgentInfo {
    w.rules
        .list_agents(viewer, Some(name.into()), 10, None)
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|a| a.name == name)
        .unwrap()
}
fn open_wait(
    w: &World,
    agent: &Agent,
    channels: Option<Vec<String>>,
) -> tokio::task::JoinHandle<Result<WaitResult>> {
    let rules = w.rules.clone();
    let agent = agent.clone();
    tokio::spawn(async move {
        rules
            .wait_for_messages(
                &agent,
                WaitQuery {
                    after_id: 0,
                    channel_ids: channels,
                    timeout: Duration::from_secs(10),
                    limit: 10,
                },
            )
            .await
    })
}
#[tokio::test]
async fn listening_covers_selected_channels_and_hides_closed_channels() {
    let w = world().await;
    let a = w.agent().await;
    let viewer = w.agent().await;
    let public = w.channel(&a, "public", true, &[]).await;
    let secret = w.channel(&a, "secret", false, &[]).await;
    let other = w.channel(&a, "other", true, &[]).await;
    let waiter = open_wait(&w, &a, Some(vec![public.clone(), secret.clone()]));
    until_sleeping(&w.clock).await;
    assert_eq!(
        observed(&w, &viewer, &a.name).await.waiting_channels,
        vec![public.clone()]
    );
    assert_eq!(observed(&w, &a, &a.name).await.waiting_channels.len(), 2);
    let members = w
        .rules
        .list_members(&viewer, other, 10, None)
        .await
        .unwrap();
    assert!(!members.page.items[0].listening);
    let members = w
        .rules
        .list_members(&viewer, public, 10, None)
        .await
        .unwrap();
    assert!(members.page.items[0].listening);
    let human = w.rules.agent_view(a.name.clone()).await.unwrap();
    assert_eq!(human.agent.waiting_channels.len(), 2);
    assert!(
        w.rules
            .overview()
            .await
            .unwrap()
            .agents
            .iter()
            .any(|x| x.info.listening)
    );
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(!observed(&w, &a, &a.name).await.listening);
}
#[tokio::test]
async fn normal_wait_completion_has_grace_but_aborted_wait_does_not() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    let first = open_wait(&w, &a, None);
    until_sleeping(&w.clock).await;
    w.clock.advance(chrono::Duration::seconds(10));
    first.await.unwrap().unwrap();
    assert!(observed(&w, &a, &a.name).await.listening);
    w.clock.advance(chrono::Duration::seconds(2));
    assert!(observed(&w, &a, &a.name).await.listening);
    let second = open_wait(&w, &a, Some(vec![ch]));
    until_sleeping(&w.clock).await;
    w.clock.advance(chrono::Duration::seconds(1));
    assert!(observed(&w, &a, &a.name).await.listening);
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    assert!(!observed(&w, &a, &a.name).await.listening);
}
#[tokio::test]
async fn overlapping_waits_and_current_memberships_are_independent() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    let first = open_wait(&w, &a, Some(vec![ch.clone()]));
    until_sleeping(&w.clock).await;
    let second = open_wait(&w, &a, None);
    while w.clock.sleepers() < 2 {
        tokio::task::yield_now().await;
    }
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(observed(&w, &a, &a.name).await.listening);
    let joined = w.channel(&a, "joined", true, &[]).await;
    assert_eq!(observed(&w, &a, &a.name).await.waiting_channels.len(), 2);
    w.rules.leave_channel(&a, ch).await.unwrap();
    assert_eq!(
        observed(&w, &a, &a.name).await.waiting_channels,
        vec![joined]
    );
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    assert!(!observed(&w, &a, &a.name).await.listening);
}

#[tokio::test]
async fn a_new_wait_supersedes_grace_and_disconnect_clears_it() {
    let w = world().await;
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    w.rules
        .wait_for_messages(
            &a,
            WaitQuery {
                after_id: 0,
                channel_ids: None,
                timeout: Duration::ZERO,
                limit: 10,
            },
        )
        .await
        .unwrap();
    assert!(observed(&w, &a, &a.name).await.listening);
    let next = open_wait(&w, &a, Some(vec![ch]));
    until_sleeping(&w.clock).await;
    next.abort();
    assert!(next.await.unwrap_err().is_cancelled());
    assert!(!observed(&w, &a, &a.name).await.listening);
}

#[tokio::test]
async fn only_the_human_administers_channels_and_every_change_is_announced() {
    let w = world().await;
    let a = w.agent().await;
    let b = w.agent().await;
    let id = w.channel(&a, "admin", false, &[]).await;
    assert_eq!(
        code(
            w.rules
                .change_channel(Actor::from(&a), id.clone(), ChannelChange::SetOpen(true))
                .await
        ),
        Code::Unauthenticated
    );
    assert_eq!(
        code(
            w.rules
                .change_channel(Actor::from(&b), id.clone(), ChannelChange::SetOpen(true))
                .await
        ),
        Code::NotFound
    );
    w.rules
        .change_channel(Actor::Human, id.clone(), ChannelChange::SetOpen(true))
        .await
        .unwrap();
    assert!(w.visible_ids(&b).await.contains(&id));
    w.rules
        .change_channel(
            Actor::Human,
            id.clone(),
            ChannelChange::AddMember(b.name.clone()),
        )
        .await
        .unwrap();
    w.rules
        .change_channel(Actor::Human, id.clone(), ChannelChange::SetOpen(false))
        .await
        .unwrap();
    assert!(w.rules.get_note(&b, id.clone()).await.is_ok());
    w.rules
        .change_channel(
            Actor::Human,
            id.clone(),
            ChannelChange::RemoveMember(b.name.clone()),
        )
        .await
        .unwrap();
    assert_eq!(code(w.read_latest(&b, &id).await), Code::NotFound);
    let messages = w.read_latest(&a, &id).await.unwrap().messages;
    assert_eq!(messages.len(), 4);
    assert!(
        messages
            .iter()
            .all(|m| m.agent == names::OPERATOR && m.run_id == Some(w.run_id))
    );
    assert!(messages.last().unwrap().body.contains(&b.name));
    // A no-op emits nothing, and an invalid member changes nothing.
    w.rules
        .change_channel(Actor::Human, id.clone(), ChannelChange::SetOpen(false))
        .await
        .unwrap();
    w.rules
        .change_channel(
            Actor::Human,
            id.clone(),
            ChannelChange::RemoveMember(b.name.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        code(
            w.rules
                .change_channel(
                    Actor::Human,
                    id.clone(),
                    ChannelChange::AddMember("missing".into())
                )
                .await
        ),
        Code::NotFound
    );
    assert_eq!(w.read_latest(&a, &id).await.unwrap().messages.len(), 4);
}

#[tokio::test]
async fn hiding_a_channel_prevents_all_agent_reads_and_keeps_stored_messages() {
    let w = world().await;
    let a = w.agent().await;
    let id = w.channel(&a, "hidden", true, &[]).await;
    let sent = w
        .rules
        .send(&a, id.clone(), "searchable history".into())
        .await
        .unwrap();
    w.rules
        .change_channel(Actor::Human, id.clone(), ChannelChange::Hide)
        .await
        .unwrap();
    assert!(!w.visible_ids(&a).await.contains(&id));
    assert_eq!(code(w.read_latest(&a, &id).await), Code::NotFound);
    assert_eq!(code(w.rules.get_message(&a, sent.id).await), Code::NotFound);
    assert_eq!(code(w.rules.get_note(&a, id.clone()).await), Code::NotFound);
    assert_eq!(
        code(w.rules.join_channel(&a, id.clone()).await),
        Code::NotFound
    );
    assert_eq!(
        code(w.rules.send(&a, id.clone(), "later".into()).await),
        Code::NotFound
    );
    assert_eq!(
        code(
            w.rules
                .change_channel(Actor::Human, id.clone(), ChannelChange::SetOpen(true))
                .await
        ),
        Code::NotFound
    );
    assert!(
        w.rules
            .search(
                &a,
                SearchQuery {
                    query: "searchable".into(),
                    channel_id: None,
                    from_agent: None,
                    limit: 20
                }
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(w.rules.activity(&a, 0).await.unwrap().is_empty());
    assert!(
        w.rules
            .wait_for_messages(
                &a,
                WaitQuery {
                    after_id: 0,
                    channel_ids: None,
                    timeout: Duration::ZERO,
                    limit: 20
                }
            )
            .await
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(w.rules.overview().await.unwrap().channels.is_empty());
    let stored_id = id.clone();
    let stored = w
        .rules
        .db()
        .read(move |s| s.channel_messages(&stored_id, None, MessageRange::Latest, 100))
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
    assert!(stored.iter().any(|m| m.body == "searchable history"));
    assert!(
        stored
            .iter()
            .any(|m| m.agent == names::OPERATOR && m.body.contains("hid the channel"))
    );
}

#[tokio::test]
async fn hiding_an_agent_revokes_it_announces_each_membership_and_keeps_its_name_reserved() {
    let w = world_with(
        Limits::DEFAULT,
        Arc::new(SameNameEntropy(SeqEntropy::new(5))),
    )
    .await;
    let registration = w.rules.register().await.unwrap();
    let a = w
        .rules
        .authenticate(Some(&registration.token))
        .await
        .unwrap();
    let b = w.agent().await;
    let first = w.channel(&b, "first", false, &[&a]).await;
    let second = w.channel(&b, "second", true, &[&a]).await;
    let sent = w
        .rules
        .send(&a, first.clone(), "kept agent message".into())
        .await
        .unwrap();
    assert_eq!(
        code(w.rules.hide_agent(Actor::from(&a), b.name.clone()).await),
        Code::Unauthenticated
    );
    w.rules
        .hide_agent(Actor::Human, a.name.clone())
        .await
        .unwrap();
    assert_eq!(
        code(w.rules.authenticate(Some(&registration.token)).await),
        Code::Unauthenticated
    );
    assert_eq!(
        code(
            w.rules
                .reclaim(a.name.clone(), registration.recovery_code)
                .await
        ),
        Code::Unauthenticated
    );
    assert_eq!(
        code(w.rules.agent_view(a.name.clone()).await),
        Code::NotFound
    );
    assert_eq!(
        code(w.rules.new_recovery_code(a.name.clone()).await),
        Code::NotFound
    );
    assert!(
        !w.rules
            .list_agents(&b, None, 100, None)
            .await
            .unwrap()
            .items
            .iter()
            .any(|x| x.name == a.name)
    );
    for id in [first, second] {
        let view = w.rules.channel_view(id.clone(), None).await.unwrap();
        assert!(!view.members.iter().any(|m| m.name == a.name));
        let last = view.messages.last().unwrap();
        assert_eq!(last.agent, names::OPERATOR);
        assert!(last.body.contains(&a.name));
        assert_eq!(
            code(w.rules.add_to_channel(&b, id, vec![a.name.clone()]).await),
            Code::NotFound
        );
    }
    assert_eq!(
        w.rules.get_message(&b, sent.id).await.unwrap().body,
        "kept agent message"
    );
    let name = a.name.clone();
    assert!(
        w.rules
            .db()
            .read(move |s| s.agent_name_taken(&name))
            .await
            .unwrap()
    );
    let new = w.agent().await;
    assert_ne!(new.name, a.name);
    assert!(new.name.starts_with(&a.name));
}

#[tokio::test]
async fn an_agent_unregisters_itself_as_the_operator_would_hide_it() {
    let w = world().await;
    let registration = w.rules.register().await.unwrap();
    let a = w
        .rules
        .authenticate(Some(&registration.token))
        .await
        .unwrap();
    let b = w.agent().await;
    let first = w.channel(&b, "first", false, &[&a]).await;
    let second = w.channel(&b, "second", true, &[&a]).await;
    let elsewhere = w.channel(&b, "elsewhere", true, &[]).await;
    let sent = w
        .rules
        .send(&a, first.clone(), "kept agent message".into())
        .await
        .unwrap();
    let mut notices = w.rules.events().subscribe();

    w.rules.unregister(&a).await.unwrap();

    // Its token and recovery code no longer work, and it is gone from lists.
    assert_eq!(
        code(w.rules.authenticate(Some(&registration.token)).await),
        Code::Unauthenticated
    );
    assert_eq!(
        code(
            w.rules
                .reclaim(a.name.clone(), registration.recovery_code)
                .await
        ),
        Code::Unauthenticated
    );
    assert!(
        !w.rules
            .list_agents(&b, None, 100, None)
            .await
            .unwrap()
            .items
            .iter()
            .any(|x| x.name == a.name)
    );
    // It left each channel it was in, saying so itself.
    for id in [&first, &second] {
        let view = w.rules.channel_view(id.clone(), None).await.unwrap();
        assert!(!view.members.iter().any(|m| m.name == a.name));
        let last = view.messages.last().unwrap();
        assert_eq!(last.agent, a.name);
        assert_eq!(
            last.body,
            format!(
                "{} unregistered and left the channel. Its messages are kept.",
                a.name
            )
        );
        assert_eq!(last.run_id, Some(w.run_id));
    }
    // A channel it was not in hears nothing.
    let view = w.rules.channel_view(elsewhere, None).await.unwrap();
    assert!(view.messages.iter().all(|m| m.agent != a.name));
    // Waiting agents hear each notice like any message, and nothing more.
    // `unregister` publishes before it returns, so both are ready now.
    let mut heard = Vec::new();
    for _ in 0..2 {
        match notices.next().now_or_never() {
            Some(Some(Notice::Message(m))) => heard.push((m.channel_id, m.agent)),
            _ => panic!("expected a ready message notice"),
        }
    }
    assert!(notices.next().now_or_never().is_none());
    heard.sort();
    let mut expected = vec![(first, a.name.clone()), (second, a.name.clone())];
    expected.sort();
    assert_eq!(heard, expected);
    // Its past messages stay.
    assert_eq!(
        w.rules.get_message(&b, sent.id).await.unwrap().body,
        "kept agent message"
    );
}

#[tokio::test]
async fn administration_rolls_back_when_the_operator_message_cannot_be_saved() {
    let w = world().await;
    let a = w.agent().await;
    let b = w.agent().await;
    let id = w.channel(&a, "atomic-admin", true, &[]).await;
    let limits = Limits::DEFAULT;
    let broken = Rules::new(Deps {
        db: w.rules.db().clone(),
        events: Events::new(),
        clock: w.clock.clone(),
        entropy: Arc::new(SeqEntropy::new(15)),
        sends: SendLimiter::new(limits.sends_per_minute),
        registrations: RegisterLimiter::new(limits.registrations_per_minute),
        waiting: Waiting::default(),
        run_id: i64::MAX,
        limits,
    });
    for change in [
        ChannelChange::SetOpen(false),
        ChannelChange::AddMember(b.name.clone()),
        ChannelChange::Hide,
    ] {
        assert_eq!(
            code(
                broken
                    .change_channel(Actor::Human, id.clone(), change)
                    .await
            ),
            Code::Internal
        );
        let view = w.rules.channel_view(id.clone(), None).await.unwrap();
        assert!(view.channel.open);
        assert_eq!(view.members.len(), 1);
        assert!(view.messages.is_empty());
    }
    assert_eq!(
        code(broken.hide_agent(Actor::Human, a.name.clone()).await),
        Code::Internal
    );
    assert!(w.rules.agent_view(a.name.clone()).await.is_ok());
    assert_eq!(
        w.rules.channel_view(id, None).await.unwrap().members.len(),
        1
    );
}

#[tokio::test]
async fn a_hidden_channel_id_is_skipped_when_entropy_draws_it_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct RepeatFirstId(AtomicUsize);
    impl Entropy for RepeatFirstId {
        fn below(&self, n: usize) -> usize {
            usize::from(self.0.fetch_add(1, Ordering::SeqCst) >= names::CHANNEL_RANDOM_LEN) % n
        }
    }
    let entropy = Arc::new(RepeatFirstId(AtomicUsize::new(0)));
    let w = world_with(Limits::DEFAULT, entropy.clone()).await;
    let a = w.agent().await;
    entropy.0.store(0, Ordering::SeqCst);
    let first = w.channel(&a, "first", true, &[]).await;
    w.rules
        .change_channel(Actor::Human, first.clone(), ChannelChange::Hide)
        .await
        .unwrap();
    entropy.0.store(0, Ordering::SeqCst);
    let second = w.channel(&a, "second", true, &[]).await;
    assert_ne!(first, second);
    assert_eq!(first, "ch_aaaaaaaa");
    assert_eq!(second, "ch_bbbbbbbb");
}

// ---- Handoff: notes and new recovery codes ----

#[tokio::test]
async fn a_handoff_note_is_set_read_back_and_cleared_by_its_identity() {
    let w = world().await;
    let (a, b) = (w.agent().await, w.agent().await);
    let fresh = w.rules.handoff_note(&a).await.unwrap();
    assert_eq!((fresh.text.as_str(), fresh.set_at), ("", None));

    let set = w
        .rules
        .set_handoff_note(&a, "  Next: the parser.\n".into())
        .await
        .unwrap();
    // Kept exactly as written; kkt doesn't define what goes in it.
    assert_eq!(set.text, "  Next: the parser.\n");
    assert_eq!(set.set_at, Some(w.rules.now_text()));
    assert_eq!(w.rules.handoff_note(&a).await.unwrap(), set);
    // Each identity reads only its own.
    assert_eq!(w.rules.handoff_note(&b).await.unwrap().text, "");
    // The human sees it on the agent's page.
    let view = w.rules.agent_view(a.name.clone()).await.unwrap();
    assert_eq!(view.handoff_note, set);

    w.clock.advance(chrono::Duration::seconds(30));
    let cleared = w.rules.set_handoff_note(&a, String::new()).await.unwrap();
    assert_eq!(cleared.text, "");
    assert_eq!(
        cleared.set_at,
        Some(w.rules.now_text()),
        "clearing is recorded"
    );
}

#[tokio::test]
async fn a_handoff_note_over_its_cap_is_refused_and_the_old_one_kept() {
    let limits = Limits {
        max_handoff_note_chars: 5,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(42))).await;
    let a = w.agent().await;
    w.rules.set_handoff_note(&a, "héllo".into()).await.unwrap();
    assert_eq!(
        code(w.rules.set_handoff_note(&a, "héllo!".into()).await),
        Code::InvalidArgument
    );
    assert_eq!(w.rules.handoff_note(&a).await.unwrap().text, "héllo");
}

#[tokio::test]
async fn a_new_recovery_code_replaces_the_old_and_hands_over_the_identity() {
    let w = world().await;
    let reg = w.rules.register().await.unwrap();
    let me = w.rules.authenticate(Some(&reg.token)).await.unwrap();
    w.rules
        .set_handoff_note(&me, "state for my successor".into())
        .await
        .unwrap();

    let code_ = w.rules.new_recovery_code(reg.name.clone()).await.unwrap();
    assert_ne!(code_, reg.recovery_code);
    // The old code stops working; the current session keeps going.
    assert_eq!(
        code(
            w.rules
                .reclaim(reg.name.clone(), reg.recovery_code.clone())
                .await
        ),
        Code::Unauthenticated
    );
    w.rules.authenticate(Some(&reg.token)).await.unwrap();

    // The new session takes over with the new code and reads the note.
    let taken = w.rules.reclaim(reg.name.clone(), code_).await.unwrap();
    assert_eq!(
        code(w.rules.authenticate(Some(&reg.token)).await),
        Code::Unauthenticated
    );
    let successor = w.rules.authenticate(Some(&taken.token)).await.unwrap();
    assert_eq!(
        w.rules.handoff_note(&successor).await.unwrap().text,
        "state for my successor"
    );
}

#[tokio::test]
async fn no_recovery_code_for_a_missing_or_revoked_agent() {
    let w = world().await;
    let a = w.agent().await;
    assert_eq!(
        code(w.rules.new_recovery_code("nobody-here".into()).await),
        Code::NotFound
    );
    w.operator.revoke(a.name.clone()).await.unwrap();
    assert_eq!(
        code(w.rules.new_recovery_code(a.name.clone()).await),
        Code::InvalidArgument
    );
}

#[tokio::test]
async fn operator_instruction_saves_are_immutable_versions_with_clock_and_run() {
    let w = world().await;
    assert_eq!(w.rules.current_operator_instructions().await.unwrap(), None);
    let first = w
        .rules
        .save_operator_instructions(Actor::Human, "  First\n😀  ".into())
        .await
        .unwrap();
    assert_eq!(first.version, 1);
    assert_eq!(first.text, "  First\n😀  ");
    assert_eq!(first.saved_at, w.rules.now_text());
    assert_eq!(first.run_id, w.run_id);
    w.clock.advance(chrono::Duration::seconds(10));
    let clear = w
        .rules
        .save_operator_instructions(Actor::Human, String::new())
        .await
        .unwrap();
    assert_eq!(clear.version, 2);
    assert_eq!(clear.saved_at, w.rules.now_text());
    assert_eq!(
        w.rules.current_operator_instructions().await.unwrap(),
        Some(clear.clone())
    );
    assert_eq!(
        w.rules.operator_instructions_history().await.unwrap(),
        vec![clear.clone(), first]
    );
    let registration = w.rules.register().await.unwrap();
    assert_eq!(registration.operator_instructions, Some(clear));
}
#[tokio::test]
async fn only_the_human_can_save_and_the_character_limit_is_enforced() {
    let limits = Limits {
        max_operator_instructions_chars: 2,
        ..Limits::DEFAULT
    };
    let w = world_with(limits, Arc::new(SeqEntropy::new(42))).await;
    let agent = w.agent().await;
    assert!(
        w.rules
            .save_operator_instructions(Actor::from(&agent), "x".into())
            .await
            .is_err()
    );
    let saved = w
        .rules
        .save_operator_instructions(Actor::Human, "😀é".into())
        .await
        .unwrap();
    assert!(
        w.rules
            .save_operator_instructions(Actor::Human, "😀éx".into())
            .await
            .is_err()
    );
    assert_eq!(
        w.rules.operator_instructions_history().await.unwrap(),
        vec![saved]
    );
}
#[tokio::test]
async fn concurrent_instruction_saves_get_distinct_increasing_versions() {
    let w = world().await;
    let (a, b) = tokio::join!(
        w.rules.save_operator_instructions(Actor::Human, "a".into()),
        w.rules.save_operator_instructions(Actor::Human, "b".into())
    );
    assert_ne!(a.unwrap().version, b.unwrap().version);
    let history = w.rules.operator_instructions_history().await.unwrap();
    assert_eq!(
        history.iter().map(|v| v.version).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[tokio::test]
async fn operator_channel_creation_is_atomic_and_never_adds_the_operator() {
    let w = world().await;
    let a = w.agent().await;
    let b = w.agent().await;
    let new = NewChannel {
        name: "  help  ".into(),
        purpose: Some("  help desk  ".into()),
        open: false,
        members: vec![a.name.clone(), a.name.clone()],
    };
    assert_eq!(
        code(
            w.rules
                .create_channel_as_operator(Actor::Agent(a.clone()), new.clone())
                .await
        ),
        Code::Unauthenticated
    );
    let created = w
        .rules
        .create_channel_as_operator(Actor::Human, new.clone())
        .await
        .unwrap();
    assert_eq!(created.name, "help");
    assert_eq!(created.created_by, names::OPERATOR);
    let view = w
        .rules
        .channel_view(created.id.clone(), None)
        .await
        .unwrap();
    assert_eq!(view.members.len(), 1);
    assert_eq!(view.members[0].name, a.name);
    assert_eq!(view.messages[0].agent, names::OPERATOR);
    assert_eq!(view.messages[0].run_id, Some(w.run_id));
    assert!(w.visible_ids(&a).await.contains(&created.id));
    assert!(!w.visible_ids(&b).await.contains(&created.id));
    let before = w.rules.overview().await.unwrap().channels.len();
    let limits = Limits::DEFAULT;
    let broken = Rules::new(Deps {
        db: w.rules.db().clone(),
        events: Events::new(),
        clock: w.clock.clone(),
        entropy: Arc::new(SeqEntropy::new(15)),
        sends: SendLimiter::new(limits.sends_per_minute),
        registrations: RegisterLimiter::new(limits.registrations_per_minute),
        waiting: Waiting::default(),
        run_id: i64::MAX,
        limits,
    });
    assert_eq!(
        code(
            broken
                .create_channel_as_operator(Actor::Human, new.clone())
                .await
        ),
        Code::Internal
    );
    let mut unknown = new;
    unknown.members.push("missing-agent".into());
    assert_eq!(
        code(
            w.rules
                .create_channel_as_operator(Actor::Human, unknown)
                .await
        ),
        Code::NotFound
    );
    assert_eq!(w.rules.overview().await.unwrap().channels.len(), before);
}
