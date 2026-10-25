//! Store tests: the SQL methods that hold logic of their own (filters, paging
//! bounds, quoting), that a failed write changes nothing, and reopening a
//! database file.

use super::sqlite::{fts_query, like_pattern};
use super::*;

const T: &str = "2026-10-23T14:00:00Z";

fn run(version: &str) -> NewServerRunRecord {
    NewServerRunRecord {
        started_at: T.into(),
        version: version.into(),
        git_commit: "abc123".into(),
        settings: "default_limit=20".into(),
        instructions_sha256: "00".into(),
    }
}

/// Agents `a`, `b` and `c`; an open channel `ch_open` ("beta", purpose
/// "parser work") with `a` and `b`; a closed channel `ch_closed` ("alpha")
/// with `a` only; and one message per (channel, agent) pair listed.
async fn world(messages: &[(&'static str, &'static str, &'static str)]) -> Db {
    let db = Db::open_in_memory().expect("db");
    let messages = messages.to_vec();
    db.write(move |w| {
        let run_id = w.insert_server_run(&run("1.0.0"))?;
        for name in ["a", "b", "c"] {
            w.insert_agent(&NewAgentRecord {
                name: name.into(),
                token_hash: name.as_bytes().to_vec(),
                recovery_hash: None,
                created_at: T.into(),
                last_seen: None,
                run_id,
            })?;
        }
        for (id, name, purpose, open) in [
            ("ch_open", "beta", Some("parser work"), true),
            ("ch_closed", "alpha", None, false),
        ] {
            w.insert_channel(&NewChannelRecord {
                id: id.into(),
                name: name.into(),
                purpose: purpose.map(str::to_string),
                open,
                created_by: "a".into(),
                created_at: T.into(),
            })?;
        }
        w.add_member("ch_open", "a", T)?;
        w.add_member("ch_open", "b", T)?;
        w.add_member("ch_closed", "a", T)?;
        for (channel_id, agent, body) in messages {
            w.insert_message(&NewMessageRecord {
                channel_id: channel_id.into(),
                agent: agent.into(),
                body: body.into(),
                created_at: T.into(),
                run_id,
            })?;
        }
        Ok::<_, StoreError>(())
    })
    .await
    .expect("seed");
    db
}

fn filter(visible_to: &str) -> ChannelFilter {
    ChannelFilter {
        visible_to: visible_to.into(),
        member_only: false,
        shared_with: None,
        text: None,
        after: None,
        limit: 100,
    }
}

async fn channel_ids(db: &Db, f: ChannelFilter) -> Vec<String> {
    db.read(move |r| r.channel_ids(&f))
        .await
        .expect("channel_ids")
}

#[test]
fn fts_query_quotes_words() {
    let words = |text: &str| {
        text.split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(fts_query(&words("parser bug")), "\"parser\" \"bug\"");
    assert_eq!(fts_query(&words("a\"b OR")), "\"a\"\"b\" \"OR\"");
}

#[test]
fn like_pattern_matches_wildcards_literally() {
    assert_eq!(like_pattern("50%_off\\"), "%50\\%\\_off\\\\%");
}

#[tokio::test]
async fn file_database_reopens_without_rerunning_migrations() {
    let dir = std::env::temp_dir().join(format!("kashkeshet-store-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("test.db");
    if path.exists() {
        std::fs::remove_file(&path).expect("remove old test database");
    }

    let db = Db::open(&path).expect("open");
    db.write(|w| {
        let run_id = w.insert_server_run(&run("1.0.0"))?;
        w.insert_agent(&NewAgentRecord {
            name: "a".into(),
            token_hash: vec![0],
            recovery_hash: Some(vec![42]),
            created_at: T.into(),
            last_seen: None,
            run_id,
        })
    })
    .await
    .expect("insert");
    drop(db);

    let db = Db::open(&path).expect("reopen");
    let found = db.read(|r| r.agent("a")).await.expect("read");
    assert_eq!(found.map(|a| a.name), Some("a".to_string()));
    assert!(
        db.read(|r| r.agent_by_recovery_hash("a", &[42]))
            .await
            .expect("recovery lookup after restart")
            .is_some()
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[tokio::test]
async fn channel_ids_show_closed_channels_to_members_only() {
    let db = world(&[]).await;
    assert_eq!(
        channel_ids(&db, filter("a")).await,
        ["ch_closed", "ch_open"]
    );
    assert_eq!(channel_ids(&db, filter("c")).await, ["ch_open"]);
}

#[tokio::test]
async fn channel_ids_filter_by_membership_sharing_and_text() {
    let db = world(&[]).await;
    let member_only = ChannelFilter {
        member_only: true,
        ..filter("b")
    };
    assert_eq!(channel_ids(&db, member_only).await, ["ch_open"]);
    let shared = ChannelFilter {
        shared_with: Some("b".into()),
        ..filter("a")
    };
    assert_eq!(channel_ids(&db, shared).await, ["ch_open"]);
    // Text matches the purpose as well as the name, and `%` is literal.
    let by_purpose = ChannelFilter {
        text: Some("parser".into()),
        ..filter("a")
    };
    assert_eq!(channel_ids(&db, by_purpose).await, ["ch_open"]);
    let wildcard = ChannelFilter {
        text: Some("%".into()),
        ..filter("a")
    };
    assert!(channel_ids(&db, wildcard).await.is_empty());
}

#[tokio::test]
async fn channel_ids_continue_after_a_name_and_id() {
    let db = world(&[]).await;
    let first = ChannelFilter {
        limit: 1,
        ..filter("a")
    };
    assert_eq!(channel_ids(&db, first).await, ["ch_closed"]);
    let rest = ChannelFilter {
        after: Some(("alpha".into(), "ch_closed".into())),
        ..filter("a")
    };
    assert_eq!(channel_ids(&db, rest).await, ["ch_open"]);
}

#[tokio::test]
async fn channel_messages_respect_range_bounds_and_order() {
    let db = world(&[
        ("ch_open", "a", "1"),
        ("ch_open", "b", "2"),
        ("ch_open", "a", "3"),
        ("ch_open", "b", "4"),
    ])
    .await;
    let ids = |range: MessageRange, from: Option<&'static str>, limit: usize| {
        let db = db.clone();
        async move {
            db.read(move |r| r.channel_messages("ch_open", from, range, limit))
                .await
                .expect("messages")
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(ids(MessageRange::Latest, None, 2).await, [4, 3]);
    assert_eq!(ids(MessageRange::After(2), None, 10).await, [3, 4]);
    assert_eq!(ids(MessageRange::Before(3), None, 10).await, [2, 1]);
    assert_eq!(ids(MessageRange::Latest, Some("a"), 10).await, [3, 1]);
}

#[tokio::test]
async fn newer_messages_cover_only_my_channels_and_the_ones_listed() {
    let db = world(&[
        ("ch_open", "a", "1"),
        ("ch_closed", "a", "2"),
        ("ch_open", "b", "3"),
    ])
    .await;
    let newer = |member: &'static str, channel_ids: Option<Vec<String>>| {
        let db = db.clone();
        async move {
            db.read(move |r| {
                let f = NewerFilter {
                    member: member.into(),
                    after_id: 1,
                    channel_ids,
                };
                Ok::<_, StoreError>((r.newer_messages(&f, 10)?, r.count_newer_messages(&f)?))
            })
            .await
            .expect("newer")
        }
    };
    let (messages, count) = newer("a", None).await;
    assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [2, 3]);
    assert_eq!(count, 2);
    let (messages, count) = newer("a", Some(vec!["ch_open".into()])).await;
    assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [3]);
    assert_eq!(count, 1);
    // `b` is not in the closed channel, so message 2 is not theirs to see.
    let (messages, _) = newer("b", None).await;
    assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [3]);
}

#[tokio::test]
async fn runs_are_listed_newest_first_and_stamp_what_they_create() {
    let db = world(&[("ch_open", "a", "hello")]).await;
    let second = db
        .write(|w| w.insert_server_run(&run("2.0.0")))
        .await
        .expect("second run");
    let runs = db.read(|r| r.server_runs()).await.expect("runs");
    assert_eq!(
        runs.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(),
        ["2.0.0", "1.0.0"]
    );
    assert_eq!(runs[0].id, second);
    let first = runs[1].id;
    let agent = db.read(|r| r.agent("a")).await.expect("agent").expect("a");
    assert_eq!(agent.run_id, Some(first));
    let message = db
        .read(|r| r.message(1))
        .await
        .expect("message")
        .expect("1");
    assert_eq!(message.run_id, Some(first));
}

#[tokio::test]
async fn name_pages_start_after_the_cursor() {
    let db = world(&[]).await;
    let names = |text: Option<&'static str>, after: Option<&'static str>, limit: usize| {
        let db = db.clone();
        async move {
            db.read(move |r| r.agents(text, after, limit))
                .await
                .expect("agents")
                .into_iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(names(None, Some("a"), 1).await, ["b"]);
    assert_eq!(names(None, Some("c"), 10).await, Vec::<String>::new());
    // `_` is literal, not a one-character wildcard.
    assert_eq!(names(Some("_"), None, 10).await, Vec::<String>::new());
    let members = db
        .read(|r| r.members_by_name("ch_open", Some("a"), 10))
        .await
        .expect("members");
    assert_eq!(members, ["b"]);
}

#[tokio::test]
async fn search_treats_query_syntax_as_words() {
    let db = world(&[
        ("ch_open", "a", "parser OR lexer"),
        ("ch_open", "b", "just the parser"),
        ("ch_closed", "a", "parser in alpha"),
    ])
    .await;
    let hits = |member: &'static str, words: &'static [&'static str]| {
        let db = db.clone();
        async move {
            db.read(move |r| {
                r.search(&SearchFilter {
                    member: member.into(),
                    words: words.iter().map(|w| w.to_string()).collect(),
                    channel_id: None,
                    from_agent: None,
                    limit: 10,
                })
            })
            .await
            .expect("search")
            .into_iter()
            .map(|h| h.message_id)
            .collect::<Vec<_>>()
        }
    };
    // `OR` is a word to find, not an operator.
    assert_eq!(hits("a", &["parser", "OR"]).await, [1]);
    assert_eq!(hits("a", &["parser"]).await, [3, 2, 1]);
    // `b` is not in the closed channel.
    assert_eq!(hits("b", &["parser"]).await, [2, 1]);
    // A stray quote or a lone operator does not break the query.
    assert!(hits("a", &["pars\"er"]).await.is_empty());
    assert!(hits("a", &["NOT"]).await.is_empty());
}

#[tokio::test]
async fn a_failed_write_changes_nothing() {
    let db = world(&[]).await;
    let failed = db
        .write(|w| {
            w.insert_channel(&NewChannelRecord {
                id: "ch_x".into(),
                name: "x".into(),
                purpose: None,
                open: true,
                created_by: "a".into(),
                created_at: T.into(),
            })?;
            w.add_member("ch_x", "a", T)?;
            Err::<(), _>(StoreError::Database("stop".into()))
        })
        .await;
    assert!(failed.is_err());
    let found = db.read(|r| r.channel("ch_x")).await.expect("read");
    assert_eq!(found, None);
    let mine = ChannelFilter {
        member_only: true,
        ..filter("a")
    };
    assert_eq!(channel_ids(&db, mine).await, ["ch_closed", "ch_open"]);
}

#[tokio::test]
async fn recovery_lookup_requires_both_name_and_hash_and_token_replacement_is_local() {
    let db = world(&[]).await;
    db.write(|w| {
        let run_id = w.insert_server_run(&run("recovery"))?;
        w.insert_agent(&NewAgentRecord {
            name: "recoverable".into(),
            token_hash: vec![11],
            recovery_hash: Some(vec![42]),
            created_at: T.into(),
            last_seen: None,
            run_id,
        })
    })
    .await
    .unwrap();
    db.write(|w| {
        assert!(w.agent_by_recovery_hash("recoverable", &[42])?.is_some());
        assert!(w.agent_by_recovery_hash("recoverable", &[41])?.is_none());
        assert!(w.agent_by_recovery_hash("a", &[42])?.is_none());
        assert!(w.agent_by_recovery_hash("a", &[])?.is_none());
        w.set_token_hash("recoverable", &[12])?;
        Ok::<_, StoreError>(())
    })
    .await
    .unwrap();
    db.read(|r| {
        assert!(r.agent_by_token_hash(&[11])?.is_none());
        assert_eq!(r.agent_by_token_hash(&[12])?.unwrap().name, "recoverable");
        assert!(r.agent_by_token_hash(b"a")?.is_some());
        assert!(r.agent_by_recovery_hash("recoverable", &[42])?.is_some());
        Ok::<_, StoreError>(())
    })
    .await
    .unwrap();
}
