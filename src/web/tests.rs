//! Page tests that every source must pass, run against the real one (over
//! the rules and an in-memory database) and the preview; plus tests of the
//! preview's simulated world and feed.

use super::*;
use crate::admission::{Admission, Sessions, WebAdmission};
use crate::clock::testing::FixedClock;
use crate::config::{Limits, PreviewSettings, WEB_CHANNEL_PAGE_MESSAGES};
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

/// What every source must do: serve each page, page back through a
/// channel, and answer 404 for what does not exist.
async fn pages_work(source: Arc<dyn Source>) {
    let overview = source.overview().await.expect("overview");
    let channel = overview.channels.first().expect("a channel").clone();
    let agent = overview.agents.first().expect("an agent").clone();
    let message = overview.recent.last().expect("a message").clone();
    let site = site(source.clone());

    let (status, page) = get(&site, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains(&format!("#{}", channel.name)),
        "front page lacks the channel"
    );
    assert!(page.contains(&agent.name), "front page lacks the agent");
    assert!(
        page.contains(&format!("{} ·", message.id)),
        "front page lacks the latest message"
    );

    let (status, page) = get(&site, &format!("/channels/{}", channel.id)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains(&format!("#{}", channel.name)));

    let view = source
        .channel(message.channel_id.clone(), None)
        .await
        .expect("channel view");
    let newest = view.messages.last().expect("a message").id;
    let (status, page) = get(
        &site,
        &format!("/channels/{}?before_id={newest}", message.channel_id),
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
    assert!(overview.channels.iter().any(|c| c.open));
    assert!(overview.channels.iter().any(|c| !c.open));
    let mut empty = 0;
    let mut paged = 0;
    let mut long = false;
    for c in &overview.channels {
        let view = p.channel(c.id.clone(), None).await.expect("view");
        if view.messages.is_empty() {
            empty += 1;
        }
        if let Some(before) = view.older_before_id {
            paged += 1;
            assert!(view.messages.len() == WEB_CHANNEL_PAGE_MESSAGES);
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
async fn the_preview_feed_has_every_kind_of_happening_and_publishes_messages() {
    let clock = Arc::new(FixedClock::new(start_time()));
    let p = preview(3, clock, &Shutdown::new());
    let mut live = p.live();
    let before = p.overview().await.expect("before");
    let (mut bursts, mut agents, mut channels, mut published) = (0, 0, 0, 0);
    for _ in 0..300 {
        match p.step() {
            Happening::Message => published += 1,
            Happening::Burst(n) => {
                bursts += 1;
                published += n;
            }
            Happening::NewAgent(_) => {
                agents += 1;
                published += 1;
            }
            Happening::NewChannel(_) => {
                channels += 1;
                published += 1;
            }
        }
    }
    assert!(bursts > 0 && agents > 0 && channels > 0);
    let after = p.overview().await.expect("after");
    assert_eq!(after.agents.len(), before.agents.len() + agents);
    assert_eq!(after.channels.len(), before.channels.len() + channels);
    // Every message was published, in order. (The buffer holds them all.)
    let mut last = 0;
    for _ in 0..published {
        match live.next().await {
            Some(LiveEvent::Message(m)) => {
                assert!(m.id > last);
                last = m.id;
            }
            other => panic!("expected a message, got {other:?}"),
        }
    }
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
    assert!(matches!(first, Some(LiveEvent::Message(_))));
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
