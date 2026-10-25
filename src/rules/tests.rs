//! Rule tests against an in-memory database. Each row of the rules table has a test.

use super::*;
use crate::clock::testing::FixedClock;
use crate::names::testing::{SameNameEntropy, SeqEntropy};
use chrono::TimeZone;
use std::time::Duration;

struct World {
    rules: Rules,
    clock: Arc<FixedClock>,
}

fn world_with(limits: Limits, entropy: Arc<dyn Entropy>) -> World {
    let clock = Arc::new(FixedClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 23, 14, 0, 0)
            .unwrap(),
    ));
    let rules = Rules::new(Deps {
        db: Db::open_in_memory().expect("db"),
        events: Events::new(),
        clock: clock.clone(),
        entropy,
        limits,
    });
    World { rules, clock }
}

fn world() -> World {
    world_with(Limits::DEFAULT, Arc::new(SeqEntropy::new(42)))
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
    let w = world();
    let reg = w.rules.register().await.unwrap();
    assert!(reg.token.starts_with("act_"));
    let me = w.rules.authenticate(Some(&reg.token)).await.unwrap();
    assert_eq!(me.name, reg.name);
}

#[tokio::test]
async fn bad_or_missing_tokens_are_unauthenticated_with_instructions() {
    let w = world();
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
    );
    let a = w.rules.register().await.unwrap();
    let b = w.rules.register().await.unwrap();
    let c = w.rules.register().await.unwrap();
    assert_eq!(b.name, format!("{}-2", a.name));
    assert_eq!(c.name, format!("{}-3", a.name));
}

#[tokio::test]
async fn last_seen_is_written_at_most_once_a_minute() {
    let w = world();
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
    assert_eq!(seen(&w).await, first);
    w.clock.advance(chrono::Duration::seconds(31));
    w.rules.authenticate(Some(&reg.token)).await.unwrap();
    assert_ne!(seen(&w).await, first);
}

#[tokio::test]
async fn list_agents_pages_by_name() {
    let w = world();
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

// ---- Rules table: appears in list_channels ----

#[tokio::test]
async fn open_channels_appear_to_everyone() {
    let w = world();
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[]).await;
    assert!(w.visible_ids(&b).await.contains(&ch));
}

#[tokio::test]
async fn closed_channels_appear_to_members_only() {
    let w = world();
    let (a, b, c) = (w.agent().await, w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[&b]).await;
    assert!(w.visible_ids(&a).await.contains(&ch));
    assert!(w.visible_ids(&b).await.contains(&ch));
    assert!(!w.visible_ids(&c).await.contains(&ch));
}

// ---- Rules table: join ----

#[tokio::test]
async fn anyone_can_join_an_open_channel() {
    let w = world();
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
    let w = world();
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
    let w = world();
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
    assert!(members.page.items.contains(&c.name));
}

#[tokio::test]
async fn any_member_can_add_others_to_a_closed_channel() {
    let w = world();
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
    let w = world();
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
    let w = world();
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
    let w = world();
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

// ---- Rules table: leave ----

#[tokio::test]
async fn any_member_can_leave_an_open_channel() {
    let w = world();
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
    let w = world();
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "secret", false, &[&b]).await;
    w.rules.leave_channel(&b, ch.clone()).await.unwrap();
    assert!(!w.visible_ids(&b).await.contains(&ch));
    assert_eq!(code(w.rules.leave_channel(&b, ch).await), Code::NotFound);
}

#[tokio::test]
async fn a_closed_channel_with_no_members_is_unreachable_but_the_human_sees_it() {
    let w = world();
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

// ---- Closed channels do not exist to outsiders ----

#[tokio::test]
async fn hidden_and_missing_channels_look_the_same() {
    let w = world();
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
    let w = world();
    let a = w.agent().await;
    let one = w.channel(&a, "general", true, &[]).await;
    let two = w.channel(&a, "general", false, &[]).await;
    assert_ne!(one, two);
}

#[tokio::test]
async fn members_can_read_history_from_before_they_joined() {
    let w = world();
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
    let w = world();
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
    let w = world();
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
    let w = world();
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
    let w = world_with(limits, Arc::new(SeqEntropy::new(1)));
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
    let w = world_with(limits, Arc::new(SeqEntropy::new(1)));
    let a = w.agent().await;
    let ch = w.channel(&a, "general", true, &[]).await;
    let sql = |sql: &'static str| {
        w.rules
            .db()
            .write(move |tx| Ok::<_, RuleError>(tx.execute_batch(sql)?))
    };
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
    let w = world();
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
    let w = world();
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
    let w = world();
    let (a, b) = (w.agent().await, w.agent().await);
    let ch = w.channel(&a, "general", true, &[]).await;
    let msg = w.rules.send(&a, ch, "hello".into()).await.unwrap();
    assert_eq!(w.rules.get_message(&a, msg.id).await.unwrap(), msg);
    assert_eq!(code(w.rules.get_message(&b, msg.id).await), Code::NotMember);
}

#[tokio::test]
async fn search_covers_only_my_channels() {
    let w = world();
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
    let w = world();
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
    let w = world();
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
    let w = world();
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
    let w = world();
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
async fn wait_times_out_by_the_injected_clock() {
    let w = world();
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
    let w = world();
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
