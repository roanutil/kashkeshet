//! Page tests that every source must pass, run against the real one (over
//! the rules and an in-memory database) and the preview; plus tests of the
//! preview's simulated world and feed.

use super::*;
use crate::admission::{Admission, Sessions, WebAdmission};
use crate::clock::testing::FixedClock;
use crate::config::{Limits, PreviewSettings};
use crate::events::Events;
use crate::names::SeededEntropy;
use crate::names::testing::SeqEntropy;
use crate::rules::{Deps, NewChannel, RegisterLimiter, Rules, SendLimiter};
use crate::shutdown::Shutdown;
use crate::store::{Db, NewServerRunRecord};
use axum::body::Body;
use axum::http::Request as HttpRequest;
use chrono::TimeZone;
use futures::StreamExt;
use preview::Happening;
use tower::ServiceExt;

fn start_time() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc
        .with_ymd_and_hms(2026, 10, 25, 12, 0, 0)
        .single()
        .expect("valid time")
}

fn open_gate() -> Arc<dyn Gate> {
    let clock = Arc::new(FixedClock::new(start_time()));
    let sessions = Sessions::new(
        clock,
        Arc::new(SeqEntropy::new(9)),
        Limits::DEFAULT.web_session_lifetime,
    );
    Arc::new(WebAdmission::new(Arc::new(Admission::Open), sessions))
}

fn site(source: Arc<dyn Source>) -> Router {
    router(
        source,
        AllowedHosts::new(&[]).expect("loopback hosts"),
        open_gate(),
    )
}

async fn get(site: &Router, path: &str) -> (StatusCode, String) {
    let request = HttpRequest::builder()
        .uri(path)
        .header("Host", "localhost")
        .body(Body::empty())
        .expect("request");
    let response = site.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A form post to `path`, sent from this site.
async fn post(site: &Router, path: &str, body: &str) -> (StatusCode, Option<String>) {
    let request = HttpRequest::builder()
        .method("POST")
        .uri(path)
        .header("Host", "localhost")
        .header("Origin", "http://localhost")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = site.clone().oneshot(request).await.expect("response");
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|l| l.to_str().ok())
        .map(str::to_string);
    (response.status(), location)
}

/// The value of the first `form_token` field in `page`.
fn form_token_in(page: &str) -> String {
    let (_, rest) = page
        .split_once(r#"name="form_token" value=""#)
        .expect("a form token");
    rest.split_once('"').expect("closing quote").0.to_string()
}

/// What every source must do for a change: refuse it without the page's
/// form token; with it, make it, and show the operator's message saying so.
async fn changes_work(source: Arc<dyn Source>) {
    let overview = source.overview().await.expect("overview");
    let id = overview
        .channels
        .first()
        .expect("a channel")
        .channel
        .id
        .clone();
    let site = site(source.clone());
    let path = format!("/channels/{id}/purpose");
    let (_, page) = get(&site, &format!("/channels/{id}")).await;
    assert!(page.contains(&format!(r#"action="/channels/{id}/purpose""#)));
    let token = form_token_in(&page);

    let (status, _) = post(&site, &path, "purpose=forged").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = post(&site, &path, "purpose=forged&form_token=guess").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let view = source.channel(id.clone(), None).await.expect("view");
    assert_ne!(view.channel.purpose.as_deref(), Some("forged"));

    let (status, location) = post(
        &site,
        &path,
        &format!("purpose=Release+%26+prep&form_token={token}"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, Some(format!("/channels/{id}")));
    let view = source.channel(id.clone(), None).await.expect("view");
    assert_eq!(view.channel.purpose.as_deref(), Some("Release & prep"));
    let said = view.messages.last().expect("the operator's message");
    assert!(said.from_operator);
    assert_eq!(said.agent, crate::names::OPERATOR);
    assert_eq!(
        said.body,
        "The operator changed the purpose to: Release & prep"
    );
    // The page shows the operator by name, with no link to an agent page.
    let (_, page) = get(&site, &format!("/channels/{id}")).await;
    assert!(page.contains(r#"<span class="who">operator</span>"#));
    assert!(!page.contains("/agents/operator"));
    // So does the front page, where the operator spoke last in the channel.
    let overview = source.overview().await.expect("overview");
    let summary = overview
        .channels
        .iter()
        .find(|c| c.channel.id == id)
        .expect("the channel");
    assert!(summary.last_from_operator);
    let (_, page) = get(&site, "/").await;
    assert!(!page.contains("/agents/operator"));

    let (status, _) = post(
        &site,
        "/channels/ch_missing0/purpose",
        &format!("purpose=x&form_token={token}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn changes_work_through_the_rules() {
    changes_work(rules_source().await).await;
}

#[tokio::test]
async fn changes_work_in_the_preview() {
    let clock = Arc::new(FixedClock::new(start_time()));
    changes_work(preview(1, clock, &Shutdown::new())).await;
}

/// What every source must do: serve each page, page back through a
/// channel, and answer 404 for what does not exist.
async fn pages_work(source: Arc<dyn Source>) {
    let overview = source.overview().await.expect("overview");
    let channel = overview
        .channels
        .first()
        .expect("a channel")
        .channel
        .clone();
    let agent = overview.agents.first().expect("an agent").agent.clone();
    let busy = overview
        .channels
        .iter()
        .find(|c| c.message_count > 0)
        .expect("a channel with messages")
        .clone();
    let site = site(source.clone());

    let (status, page) = get(&site, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains(&format!("#{}", channel.name)),
        "front page lacks the channel"
    );
    assert!(page.contains(&agent.name), "front page lacks the agent");
    assert!(page.contains(&format!("data-now=\"{}\"", overview.now)));
    assert!(
        page.contains(&format!("data-channel-id=\"{}\"", busy.channel.id)),
        "front page lacks a channel row"
    );
    assert!(
        !page.contains("<li class=\"msg\""),
        "the front page lists no messages"
    );

    let (status, page) = get(&site, &format!("/channels/{}", channel.id)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains(&format!("#{}", channel.name)));
    assert!(page.contains("data-follow"), "the newest page is live");
    assert!(page.contains(&format!("data-max=\"{}\"", source.page_messages())));

    let view = source
        .channel(busy.channel.id.clone(), None)
        .await
        .expect("channel view");
    let newest = view.messages.last().expect("a message").id;
    let (status, page) = get(
        &site,
        &format!("/channels/{}?before_id={newest}", busy.channel.id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains("Latest messages"),
        "a page of older messages links back"
    );

    let (status, page) = get(&site, &format!("/agents/{}", agent.name)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains(&agent.name));

    // An agent's description shows on the front page, its own page, and as
    // the title of its chip on the pages of channels it is in.
    let described = overview
        .agents
        .iter()
        .find_map(|a| Some((a, a.agent.description.clone()?)))
        .expect("an agent with a description");
    let (a, text) = described;
    let (_, page) = get(&site, "/").await;
    assert!(page.contains(&text), "front page lacks a description");
    let (_, page) = get(&site, &format!("/agents/{}", a.agent.name)).await;
    assert!(page.contains(&text), "agent page lacks its description");
    if let Some(c) = a.channels.first() {
        let (_, page) = get(&site, &format!("/channels/{}", c.id)).await;
        assert!(
            page.contains(&format!("title=\"{text}\"")),
            "channel page lacks a member's description"
        );
    }

    let (status, _) = get(&site, "/channels/ch_nothere0").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&site, "/agents/nobody-here").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Rules over an in-memory database with two agents, an open and a closed
/// channel, and a few messages.
async fn rules_source() -> Arc<dyn Source> {
    let db = Db::open_in_memory().expect("db");
    let run_id = db
        .write(|w| {
            w.insert_server_run(&NewServerRunRecord {
                started_at: "2026-10-25T12:00:00Z".into(),
                version: "test".into(),
                git_commit: "test".into(),
                settings: "test".into(),
                instructions_sha256: "test".into(),
            })
        })
        .await
        .expect("record run");
    let rules = Rules::new(Deps {
        waiting: crate::rules::Waiting::default(),
        db,
        events: Events::new(),
        clock: Arc::new(FixedClock::new(start_time())),
        entropy: Arc::new(SeqEntropy::new(4)),
        sends: SendLimiter::new(Limits::DEFAULT.sends_per_minute),
        registrations: RegisterLimiter::new(Limits::DEFAULT.registrations_per_minute),
        run_id,
        limits: Limits::DEFAULT,
    });
    let mut agents = Vec::new();
    for _ in 0..2 {
        let reg = rules.register().await.expect("register");
        agents.push(rules.authenticate(Some(&reg.token)).await.expect("auth"));
    }
    rules
        .set_description(&agents[0], "Codex, reviews only".into())
        .await
        .expect("describe");
    for (name, open) in [("open-room", true), ("closed-room", false)] {
        let channel = rules
            .create_channel(
                &agents[0],
                NewChannel {
                    name: name.into(),
                    open,
                    purpose: None,
                    members: vec![agents[1].name.clone()],
                },
            )
            .await
            .expect("channel");
        rules
            .set_note(
                &agents[1],
                channel.id.clone(),
                "First line\n<script>alert(1)</script>".into(),
            )
            .await
            .expect("note");
        for i in 0..3 {
            rules
                .send(&agents[i % 2], channel.id.clone(), format!("message {i}"))
                .await
                .expect("send");
        }
    }
    Arc::new(RulesSource::new(rules))
}

fn preview(seed: u64, clock: Arc<FixedClock>, shutdown: &Shutdown) -> Arc<Preview> {
    Arc::new(Preview::new(
        clock,
        Arc::new(SeededEntropy::new(seed)),
        PreviewSettings::DEFAULT,
        shutdown,
    ))
}

#[tokio::test]
async fn pages_work_from_the_rules() {
    pages_work(rules_source().await).await;
}

#[tokio::test]
async fn pages_work_from_the_preview() {
    let clock = Arc::new(FixedClock::new(start_time()));
    pages_work(preview(1, clock, &Shutdown::new())).await;
}

#[tokio::test]
async fn the_show_older_link_appears_only_when_older_messages_exist() {
    let clock = Arc::new(FixedClock::new(start_time()));
    let p = preview(1, clock, &Shutdown::new());
    let site = site(p.clone());
    let overview = p.overview().await.expect("overview");
    let mut seen_paged = false;
    let mut seen_single = false;
    for c in overview.channels.iter().map(|s| &s.channel) {
        let view = p.channel(c.id.clone(), None).await.expect("view");
        let (status, page) = get(&site, &format!("/channels/{}", c.id)).await;
        assert_eq!(status, StatusCode::OK);
        match view.older_before_id {
            Some(before) => {
                seen_paged = true;
                assert!(
                    page.contains(&format!("?before_id={before}\">Show older")),
                    "{}",
                    c.name
                );
                assert!(!page.contains("id=\"older\" hidden"), "{}", c.name);
            }
            None => {
                seen_single = true;
                assert!(page.contains("id=\"older\" hidden"), "{}", c.name);
            }
        }
    }
    assert!(seen_paged && seen_single);
}

/// The front page's summary must agree with what the channel and agent
/// pages show, for any source.
async fn summaries_agree_with_the_pages(source: Arc<dyn Source>) {
    let overview = source.overview().await.expect("overview");
    for c in &overview.channels {
        let view = source
            .channel(c.channel.id.clone(), None)
            .await
            .expect("view");
        let newest = view.messages.last();
        assert_eq!(c.last_message_at, newest.map(|m| m.created_at.clone()));
        assert_eq!(c.last_speaker, newest.map(|m| m.agent.clone()));
        assert!(c.message_count >= view.messages.len() as i64);
        assert_eq!(c.message_count == 0, view.messages.is_empty());
        assert_eq!(c.channel.member_count, view.members.len() as i64);
        let starts: Vec<&str> = c.recent_activity.iter().map(|a| a.start.as_str()).collect();
        let mut sorted = starts.clone();
        sorted.sort();
        assert_eq!(sorted, starts, "recent counts are oldest first");
        for a in &c.recent_activity {
            assert_eq!(
                crate::clock::floor_text(&a.start, crate::config::WEB_ACTIVITY_BUCKET).as_deref(),
                Some(a.start.as_str()),
                "stretches start on a stretch boundary"
            );
        }
        // With every message on one page, the recent count is exact.
        if view.older_before_id.is_none() {
            let since = crate::clock::to_text(
                crate::clock::from_text(&overview.now).expect("now")
                    - chrono::Duration::from_std(crate::config::WEB_ACTIVITY_WINDOW)
                        .expect("window"),
            );
            let in_window = view
                .messages
                .iter()
                .filter(|m| m.created_at >= since)
                .count();
            assert_eq!(c.recent_count(), in_window as i64, "{}", c.channel.name);
        }
    }
    for a in &overview.agents {
        let view = source.agent(a.agent.name.clone()).await.expect("view");
        let mut theirs: Vec<&str> = view.channels.iter().map(|c| c.id.as_str()).collect();
        let mut listed: Vec<&str> = a.channels.iter().map(|c| c.id.as_str()).collect();
        theirs.sort();
        listed.sort();
        assert_eq!(listed, theirs, "{}", a.agent.name);
    }
    // The front page keeps channels and agents in creation order.
    let (_, page) = get(&site(source), "/").await;
    let rows: Vec<&str> = page
        .split("<tr data-channel-id=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    let mut channels = overview.channels.clone();
    channels.sort_by(|a, b| {
        (&a.channel.created_at, &a.channel.id).cmp(&(&b.channel.created_at, &b.channel.id))
    });
    let expected: Vec<&str> = channels.iter().map(|c| c.channel.id.as_str()).collect();
    assert_eq!(rows, expected);
    let agent_rows: Vec<&str> = page
        .split("<tr data-agent=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    let mut agents = overview.agents.clone();
    agents.sort_by(|a, b| {
        (&a.agent.created_at, &a.agent.name).cmp(&(&b.agent.created_at, &b.agent.name))
    });
    let expected: Vec<&str> = agents.iter().map(|a| a.agent.name.as_str()).collect();
    assert_eq!(agent_rows, expected);
}

#[tokio::test]
async fn summaries_agree_with_the_pages_from_the_rules() {
    summaries_agree_with_the_pages(rules_source().await).await;
}

#[tokio::test]
async fn summaries_agree_with_the_pages_from_the_preview() {
    let clock = Arc::new(FixedClock::new(start_time()));
    let p = preview(4, clock, &Shutdown::new());
    for _ in 0..30 {
        p.step();
    }
    summaries_agree_with_the_pages(p).await;
}

#[tokio::test]
async fn the_preview_counts_recent_messages_in_the_window() {
    let clock = Arc::new(FixedClock::new(start_time()));
    let p = preview(1, clock.clone(), &Shutdown::new());
    // The starting history ends now, so some of it is recent.
    let before: i64 = p
        .overview()
        .await
        .expect("overview")
        .channels
        .iter()
        .map(|c| c.recent_count())
        .sum();
    assert!(before > 0);
    // Ten minutes later, with nothing new, nothing is recent.
    clock.advance(chrono::Duration::minutes(10));
    let after = p.overview().await.expect("overview");
    assert!(after.channels.iter().all(|c| c.recent_activity.is_empty()));
    assert_eq!(
        after.now,
        crate::clock::to_text(start_time() + chrono::Duration::minutes(10))
    );
    // A new message is recent again.
    p.step();
    let again: i64 = p
        .overview()
        .await
        .expect("overview")
        .channels
        .iter()
        .map(|c| c.recent_count())
        .sum();
    assert!(again >= 1);
}

#[tokio::test]
async fn the_preview_world_repeats_from_its_seed() {
    let shutdown = Shutdown::new();
    let world = |seed| {
        let clock = Arc::new(FixedClock::new(start_time()));
        let p = preview(seed, clock, &shutdown);
        for _ in 0..50 {
            p.step();
        }
        p
    };
    let a = world(5).overview().await.expect("a");
    let b = world(5).overview().await.expect("b");
    let c = world(6).overview().await.expect("c");
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[tokio::test]
async fn the_preview_world_has_what_the_page_must_show() {
    let clock = Arc::new(FixedClock::new(start_time()));
    let p = preview(1, clock, &Shutdown::new());
    let overview = p.overview().await.expect("overview");
    assert!(overview.channels.iter().any(|c| c.channel.open));
    assert!(overview.channels.iter().any(|c| !c.channel.open));
    let mut empty = 0;
    let mut paged = 0;
    let mut long = false;
    for c in overview.channels.iter().map(|s| &s.channel) {
        let view = p.channel(c.id.clone(), None).await.expect("view");
        if view.messages.is_empty() {
            empty += 1;
        }
        if let Some(before) = view.older_before_id {
            paged += 1;
            assert_eq!(view.messages.len(), p.page_messages());
            let older = p.channel(c.id.clone(), Some(before)).await.expect("older");
            assert!(older.messages.iter().all(|m| m.id < before));
        }
        long |= view.messages.iter().any(|m| m.body.len() > 500);
    }
    assert_eq!(empty, 1, "one channel starts empty");
    assert!(paged >= 1, "a channel has more than one page");
    assert!(long, "some messages are very long");
}

#[tokio::test]
async fn the_preview_feed_has_every_kind_of_happening_and_publishes_it() {
    let clock = Arc::new(FixedClock::new(start_time()));
    let p = preview(3, clock, &Shutdown::new());
    let mut live = p.live();
    let before = p.overview().await.expect("before");
    let (mut bursts, mut agents, mut channels, mut messages) = (0, 0, 0, 0);
    for _ in 0..200 {
        match p.step() {
            Happening::Message => messages += 1,
            Happening::Burst(n) => {
                bursts += 1;
                messages += n;
            }
            // A new agent says hello; a new channel gets a first message.
            Happening::NewAgent(_) => {
                agents += 1;
                messages += 1;
            }
            Happening::NewChannel(_) => {
                channels += 1;
                messages += 1;
            }
        }
    }
    assert!(bursts > 0 && agents > 0 && channels > 0);
    let after = p.overview().await.expect("after");
    assert_eq!(after.agents.len(), before.agents.len() + agents);
    assert_eq!(after.channels.len(), before.channels.len() + channels);

    // Everything was published, in order. (The buffer holds it all.)
    let (mut heard_messages, mut heard_agents, mut heard_channels, mut heard_joins) = (0, 0, 0, 0);
    let mut last = 0;
    let mut joined_new_agent = true;
    let mut newest_agent = String::new();
    while heard_messages < messages {
        match live.next().await {
            Some(LiveEvent::Message(m)) => {
                assert!(m.id > last);
                last = m.id;
                heard_messages += 1;
            }
            Some(LiveEvent::Agent(a)) => {
                newest_agent = a.name;
                heard_agents += 1;
            }
            Some(LiveEvent::Channel(_)) => heard_channels += 1,
            Some(LiveEvent::Joined(j)) => {
                joined_new_agent &= j.agent == newest_agent;
                heard_joins += 1;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(heard_agents, agents);
    assert_eq!(heard_channels, channels);
    assert!(
        heard_joins >= agents,
        "each new agent joins at least one channel"
    );
    assert!(joined_new_agent, "joins follow the agent that joined");
}

#[tokio::test]
async fn the_preview_runs_by_the_clock_and_stops_with_the_server() {
    let clock = Arc::new(FixedClock::new(start_time()));
    let shutdown = Shutdown::new();
    let p = preview(2, clock.clone(), &shutdown);
    let mut live = p.live();
    let runner = {
        let p = p.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move { p.run(shutdown).await })
    };
    // At 30 steps a minute, one step every two seconds.
    tokio::task::yield_now().await;
    clock.advance(chrono::Duration::seconds(2));
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), live.next())
        .await
        .expect("a step after two seconds");
    assert!(
        matches!(first, Some(ref e) if !matches!(e, LiveEvent::Missed(_))),
        "{first:?}"
    );
    shutdown.stop();
    tokio::time::timeout(std::time::Duration::from_secs(5), runner)
        .await
        .expect("the preview stops")
        .expect("no panic");
    assert!(
        live.next().await.is_none(),
        "live feeds end with the server"
    );
}

#[tokio::test]
async fn channel_page_shows_the_pinned_note_and_editor_as_escaped_text() {
    let source = rules_source().await;
    let overview = source.overview().await.unwrap();
    let ch = &overview.channels[0].channel;
    let (status, page) = get(&site(source.clone()), &format!("/channels/{}", ch.id)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("Pinned note"));
    assert!(
        page.contains("First line\n&#60;script&#62;alert(1)&#60;/script&#62;"),
        "{page}"
    );
    assert!(!page.contains("<script>alert(1)</script>"));
    assert!(page.contains("Last edited by"));
    assert!(page.contains(ch.note_edited_by.as_deref().unwrap()));
    assert!(page.contains("2026-10-25"));
}

async fn administration_works(source: Arc<dyn Source>) {
    let overview = source.overview().await.unwrap();
    let id = overview.channels[0].channel.id.clone();
    let current = source.channel(id.clone(), None).await.unwrap();
    let name = current.members[0].name.clone();
    let count = current.members.len();
    let app = site(source.clone());
    let (_, page) = get(&app, &format!("/channels/{id}")).await;
    let token = form_token_in(&page);
    let routes = [
        (
            format!("/channels/{id}/visibility"),
            "open=false".to_string(),
        ),
        (
            format!("/channels/{id}/members/add"),
            format!("name={name}"),
        ),
        (
            format!("/channels/{id}/members/remove"),
            format!("name={name}"),
        ),
        (format!("/channels/{id}/hide"), String::new()),
        (format!("/agents/{name}/hide"), String::new()),
    ];
    for (path, fields) in &routes {
        assert_eq!(post(&app, path, fields).await.0, StatusCode::FORBIDDEN);
        assert_eq!(
            post(&app, path, &format!("{fields}&form_token=invalid"))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(get(&app, path).await.0, StatusCode::METHOD_NOT_ALLOWED);
        let request = HttpRequest::builder()
            .method("POST")
            .uri(path)
            .header("Host", "localhost")
            .header("Origin", "http://other.example")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("{fields}&form_token={token}")))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    // The forged requests made no changes.
    assert_eq!(
        source
            .channel(id.clone(), None)
            .await
            .unwrap()
            .members
            .len(),
        count
    );
    assert!(source.agent(name.clone()).await.is_ok());
    let open = !current.channel.open;
    assert_eq!(
        post(
            &app,
            &routes[0].0,
            &format!("open={open}&form_token={token}")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        source.channel(id.clone(), None).await.unwrap().channel.open,
        open
    );
    assert_eq!(
        post(
            &app,
            &routes[2].0,
            &format!("name={name}&form_token={token}")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        source
            .channel(id.clone(), None)
            .await
            .unwrap()
            .members
            .len(),
        count - 1
    );
    assert_eq!(
        post(
            &app,
            &routes[1].0,
            &format!("name={name}&form_token={token}")
        )
        .await
        .0,
        StatusCode::SEE_OTHER
    );
    let view = source.channel(id.clone(), None).await.unwrap();
    assert_eq!(view.members.len(), count);
    assert!(view.messages.last().unwrap().from_operator);
    assert!(view.messages.last().unwrap().body.contains("added"));
    assert_eq!(
        post(&app, &routes[4].0, &format!("form_token={token}"))
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert!(matches!(
        source.agent(name.clone()).await,
        Err(SourceError::NotFound(_))
    ));
    let view = source.channel(id.clone(), None).await.unwrap();
    assert!(!view.members.iter().any(|m| m.name == name));
    assert!(view.messages.last().unwrap().from_operator);
    assert!(view.messages.last().unwrap().body.contains("hid"));
    assert_eq!(
        post(&app, &routes[3].0, &format!("form_token={token}")).await,
        (StatusCode::SEE_OTHER, Some("/".into()))
    );
    assert!(matches!(
        source.channel(id.clone(), None).await,
        Err(SourceError::NotFound(_))
    ));
    let overview = source.overview().await.unwrap();
    assert!(!overview.channels.iter().any(|c| c.channel.id == id));
    assert!(!overview.agents.iter().any(|a| a.agent.name == name));
}

#[tokio::test]
async fn administration_works_through_the_rules() {
    administration_works(rules_source().await).await;
}

#[tokio::test]
async fn administration_works_through_the_preview() {
    administration_works(preview(
        1,
        Arc::new(FixedClock::new(start_time())),
        &Shutdown::new(),
    ))
    .await;
}

#[tokio::test]
async fn preview_feed_survives_empty_channels_and_hiding_every_entry() {
    let p = preview(1, Arc::new(FixedClock::new(start_time())), &Shutdown::new());
    let overview = p.overview().await.unwrap();
    for agent in overview.agents {
        p.hide_agent(agent.agent.name).await.unwrap();
    }
    // No remaining channel members must not cause an entropy-index panic.
    p.step();
    for channel in p.overview().await.unwrap().channels {
        p.change_channel(channel.channel.id, ChannelAction::Hide)
            .await
            .unwrap();
    }
    assert!(p.overview().await.unwrap().channels.is_empty());
    p.step();
    assert!(!p.overview().await.unwrap().channels.is_empty());
}
