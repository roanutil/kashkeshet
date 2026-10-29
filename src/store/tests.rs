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
    assert_eq!(
        members.into_iter().map(|a| a.name).collect::<Vec<_>>(),
        ["b"]
    );
}

#[tokio::test]
async fn descriptions_are_set_cleared_and_listed_with_members() {
    let db = world(&[]).await;
    db.write(|w| w.set_description("b", Some("parser bot")))
        .await
        .expect("set");
    let members = db
        .read(|r| r.members_by_name("ch_open", None, 10))
        .await
        .expect("members");
    let described: Vec<_> = members
        .iter()
        .map(|a| (a.name.as_str(), a.description.as_deref()))
        .collect();
    assert_eq!(described, [("a", None), ("b", Some("parser bot"))]);
    db.write(|w| w.set_description("b", None))
        .await
        .expect("clear");
    let b = db.read(|r| r.agent("b")).await.expect("agent").expect("b");
    assert_eq!(b.description, None);
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

#[tokio::test]
async fn channel_use_counts_messages_and_names_the_last_speaker() {
    let db = world(&[
        ("ch_open", "a", "one"),
        ("ch_open", "b", "two"),
        ("ch_open", "a", "three"),
        ("ch_closed", "a", "solo"),
    ])
    .await;
    // A channel with no messages is listed too.
    db.write(|w| {
        w.insert_channel(&NewChannelRecord {
            id: "ch_quiet".into(),
            name: "quiet".into(),
            purpose: None,
            open: true,
            created_by: "c".into(),
            created_at: T.into(),
        })?;
        w.insert_message(&NewMessageRecord {
            channel_id: "ch_open".into(),
            agent: "b".into(),
            body: "four".into(),
            created_at: "2026-10-23T14:05:00Z".into(),
            run_id: 1,
        })?;
        Ok::<_, StoreError>(())
    })
    .await
    .expect("seed");
    let mut used = db.read(|r| r.channel_use()).await.expect("read");
    used.sort_by(|x, y| x.channel_id.cmp(&y.channel_id));
    let at = |s: &str| Some(s.to_string());
    assert_eq!(
        used,
        [
            ChannelUseRecord {
                channel_id: "ch_closed".into(),
                message_count: 1,
                last_message_at: at(T),
                last_speaker: at("a"),
            },
            ChannelUseRecord {
                channel_id: "ch_open".into(),
                message_count: 4,
                last_message_at: at("2026-10-23T14:05:00Z"),
                last_speaker: at("b"),
            },
            ChannelUseRecord {
                channel_id: "ch_quiet".into(),
                message_count: 0,
                last_message_at: None,
                last_speaker: None,
            },
        ]
    );
}

#[tokio::test]
async fn memberships_list_every_agent_in_every_channel() {
    let db = world(&[]).await;
    let all = db.read(|r| r.memberships()).await.expect("read");
    let pairs: Vec<(String, String, String)> = all
        .into_iter()
        .map(|m| (m.channel_id, m.channel_name, m.agent))
        .collect();
    let pair = |c: &str, n: &str, a: &str| (c.to_string(), n.to_string(), a.to_string());
    assert_eq!(
        pairs,
        [
            pair("ch_open", "beta", "a"),
            pair("ch_open", "beta", "b"),
            pair("ch_closed", "alpha", "a"),
        ]
    );
}

#[tokio::test]
async fn message_times_since_finds_every_message_in_the_window() {
    let db = world(&[("ch_open", "a", "old one"), ("ch_closed", "a", "old two")]).await;
    db.write(|w| {
        for (channel_id, at) in [
            ("ch_open", "2026-10-23T14:10:00Z"),
            ("ch_closed", "2026-10-23T14:11:00Z"),
            ("ch_open", "2026-10-23T14:12:00Z"),
            // Stored after the others but stamped earlier, as when the clock
            // was read before the write got its turn, or the clock moved
            // back. It must not hide the rest.
            ("ch_closed", "2026-10-23T13:00:00Z"),
        ] {
            w.insert_message(&NewMessageRecord {
                channel_id: channel_id.into(),
                agent: "a".into(),
                body: "new".into(),
                created_at: at.into(),
                run_id: 1,
            })?;
        }
        Ok::<_, StoreError>(())
    })
    .await
    .expect("seed");
    let times = db
        .read(|r| r.message_times_since("2026-10-23T14:10:00Z"))
        .await
        .expect("read");
    let pairs: Vec<(&str, &str)> = times
        .iter()
        .map(|t| (t.channel_id.as_str(), t.created_at.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("ch_open", "2026-10-23T14:10:00Z"),
            ("ch_closed", "2026-10-23T14:11:00Z"),
            ("ch_open", "2026-10-23T14:12:00Z"),
        ]
    );
    let none = db
        .read(|r| r.message_times_since("2026-10-23T15:00:00Z"))
        .await
        .expect("read");
    assert!(none.is_empty());
}

/// A fresh, empty directory for one test's files.
fn scratch_dir(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kashkeshet-store-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copies_taken_during_writes_are_whole_and_consistent() {
    const MESSAGES: i64 = 400;
    let dir = scratch_dir("copy");
    let path = dir.join("live.db");
    let db = Db::open(&path).expect("open");
    db.write(|w| {
        let run_id = w.insert_server_run(&run("1.0.0"))?;
        w.insert_agent(&NewAgentRecord {
            name: "a".into(),
            token_hash: vec![0],
            recovery_hash: None,
            created_at: T.into(),
            last_seen: None,
            run_id,
        })?;
        w.insert_channel(&NewChannelRecord {
            id: "ch_1".into(),
            name: "general".into(),
            purpose: None,
            open: true,
            created_by: "a".into(),
            created_at: T.into(),
        })
    })
    .await
    .expect("seed");

    let writer = {
        let db = db.clone();
        tokio::spawn(async move {
            for i in 0..MESSAGES {
                db.write(move |w| {
                    w.insert_message(&NewMessageRecord {
                        channel_id: "ch_1".into(),
                        agent: "a".into(),
                        body: format!("message {i}"),
                        created_at: T.into(),
                        run_id: 1,
                    })
                })
                .await
                .expect("insert");
            }
        })
    };
    let copies = {
        let (path, dir) = (path.clone(), dir.clone());
        tokio::task::spawn_blocking(move || {
            (0..8)
                .map(|i| {
                    let copy = dir.join(format!("copy-{i}.db"));
                    copy_database(&path, &copy).expect("copy");
                    copy
                })
                .collect::<Vec<_>>()
        })
    };
    let copies = copies.await.expect("copies");
    writer.await.expect("writer");

    let mut counts = Vec::new();
    for copy in &copies {
        assert!(!std::path::PathBuf::from(format!("{}-wal", copy.display())).exists());
        let conn = rusqlite::Connection::open(copy).expect("open copy");
        let check: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .expect("integrity check");
        assert_eq!(check, "ok");
        // Each message was its own transaction, so a consistent copy holds
        // the first n and nothing else.
        let (count, highest): (i64, Option<i64>) = conn
            .query_row("SELECT count(*), max(id) FROM messages", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .expect("count");
        assert!(count <= MESSAGES);
        assert_eq!(highest.unwrap_or(0), count);
        counts.push(count);
        drop(conn);
        let reopened = Db::open(copy).expect("a copy opens as a database");
        let agent = reopened.read(|r| r.agent("a")).await.expect("read copy");
        assert!(agent.is_some());
    }
    assert!(
        counts.iter().any(|&n| n > 0 && n < MESSAGES),
        "no copy was taken while messages were being written: {counts:?}"
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_copy_never_replaces_an_existing_file() {
    let dir = scratch_dir("copy-exists");
    let path = dir.join("live.db");
    drop(Db::open(&path).expect("open"));
    let copy = dir.join("copy.db");
    std::fs::write(&copy, "keep me").expect("existing file");
    assert!(copy_database(&path, &copy).is_err());
    assert_eq!(std::fs::read_to_string(&copy).expect("read"), "keep me");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
