//! Output tests: message formatting, the size cap, and the shared helpers.

use super::fit::{cut_body, cut_bytes, short_time};
use super::*;
use crate::clock::testing::FixedClock;
use crate::config::Limits;
use crate::rules::{AgentInfo, Members, Message, MessagePage, Page, Position};
use chrono::{TimeZone, Utc};
use serde_json::Value;

fn formatter(max: usize) -> Formatter {
    let now = Utc.with_ymd_and_hms(2026, 10, 23, 15, 0, 0).unwrap();
    Formatter::new(max, Arc::new(FixedClock::new(now)))
}

fn msg(id: i64, body: &str) -> Message {
    Message {
        id,
        channel_id: "ch_7k2m9q4x".into(),
        channel_name: "general".into(),
        agent: "amber-falcon".into(),
        body: body.into(),
        created_at: "2026-10-23T14:02:00Z".into(),
        run_id: None,
    }
}

#[test]
fn text_messages_look_like_the_plan() {
    let page = MessagePage {
        messages: vec![
            msg(1201, "can someone take the parser?"),
            msg(1207, &"x".repeat(600)),
        ],
        more: Some(Position::Before(1201)),
    };
    let out = formatter(8192).read(Format::Text, &page, Position::Latest, 500);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "#general [ch_7k2m9q4x]");
    assert_eq!(
        lines[1],
        "[1201 14:02 amber-falcon] can someone take the parser?"
    );
    assert!(lines[2].ends_with("…  (cut; get_message 1207)"));
    assert_eq!(lines[3], "more: before_id=1201");
}

#[test]
fn size_cap_keeps_newest_and_points_back() {
    let msgs: Vec<Message> = (1..=50).map(|i| msg(i, &"y".repeat(300))).collect();
    let page = MessagePage {
        messages: msgs,
        more: None,
    };
    let out = formatter(2000).read(Format::Text, &page, Position::Latest, 500);
    assert!(out.len() <= 2000);
    let first_id: i64 = out.lines().nth(1).unwrap()[1..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(out.contains(&format!("more: before_id={first_id}")));
    assert!(out.contains("[50 "));
}

#[test]
fn size_cap_keeps_oldest_when_reading_forward() {
    let msgs: Vec<Message> = (1..=50).map(|i| msg(i, &"y".repeat(300))).collect();
    let page = MessagePage {
        messages: msgs,
        more: None,
    };
    let out = formatter(2000).read(Format::Json, &page, Position::After(0), 500);
    assert!(out.len() <= 2000);
    let v: Value = serde_json::from_str(&out).unwrap();
    let shown = v["messages"].as_array().unwrap();
    let last = shown.last().unwrap()["id"].as_i64().unwrap();
    assert_eq!(shown[0]["id"], 1);
    assert_eq!(v["more"], format!("after_id={last}"));
}

#[test]
fn a_message_too_big_for_any_response_points_to_get_message() {
    let page = MessagePage {
        messages: vec![msg(9, &"z".repeat(5000))],
        more: None,
    };
    let out = formatter(1000).read(Format::Text, &page, Position::Latest, 8000);
    assert!(out.len() <= 1000);
    assert!(out.contains("get_message 9"));
}

#[test]
fn lists_cut_at_cap_with_cursor() {
    let page = Page {
        items: (0..100)
            .map(|i| AgentInfo {
                name: format!("agent-{i:03}"),
                created_at: "2026-10-23T14:00:00Z".into(),
                last_seen: None,
                run_id: None,
                description: None,
            })
            .collect(),
        next_cursor: None,
    };
    let out = formatter(200).agents(Format::Text, &page);
    assert!(out.len() <= 200);
    let last_shown = out.lines().rev().nth(1).unwrap();
    assert_eq!(
        out.lines().last().unwrap(),
        format!("more: cursor={last_shown}")
    );
}

fn described(name: &str, description: Option<&str>) -> AgentInfo {
    AgentInfo {
        name: name.into(),
        created_at: "2026-10-23T14:00:00Z".into(),
        last_seen: Some("2026-10-23T14:05:00Z".into()),
        run_id: None,
        description: description.map(str::to_string),
    }
}

#[test]
fn agent_and_member_lists_show_descriptions() {
    let items = vec![
        described("amber-falcon", Some("Codex, reviews only")),
        described("brave-beaver", None),
    ];
    let page = Page {
        items: items.clone(),
        next_cursor: None,
    };
    assert_eq!(
        formatter(8000).agents(Format::Text, &page),
        "amber-falcon  last seen 14:05 — Codex, reviews only\nbrave-beaver  last seen 14:05"
    );
    let json: Value = serde_json::from_str(&formatter(8000).agents(Format::Json, &page)).unwrap();
    assert_eq!(json["agents"][0]["description"], "Codex, reviews only");
    assert_eq!(json["agents"][1]["description"], Value::Null);

    let members = Members {
        channel_id: "ch_7k2m9q4x".into(),
        total: 2,
        page: Page {
            items,
            next_cursor: None,
        },
    };
    assert_eq!(
        formatter(8000).members(Format::Text, &members),
        "ch_7k2m9q4x: 2 members\namber-falcon — Codex, reviews only\nbrave-beaver"
    );
    let json: Value =
        serde_json::from_str(&formatter(8000).members(Format::Json, &members)).unwrap();
    assert_eq!(json["members"][0]["name"], "amber-falcon");
    assert_eq!(json["members"][0]["description"], "Codex, reviews only");
    assert_eq!(json["members"][1]["description"], Value::Null);
}

#[test]
fn the_longest_descriptions_fit_the_smallest_response_cap() {
    let cap = crate::config::MIN_RESPONSE_BYTES;
    let max_chars = Limits::DEFAULT.max_description_chars;
    // Four-byte characters, characters JSON must escape, and control
    // characters JSON writes as six bytes each.
    for long in [
        "😀".repeat(max_chars),
        "\"\\".repeat(max_chars / 2),
        "\u{1}\n".repeat(max_chars / 2),
    ] {
        let agent = described("amber-falcon", Some(&long));
        let fmt = formatter(cap);
        for f in [Format::Text, Format::Json] {
            let reply = fmt.description(f, &agent);
            assert!(reply.len() <= cap, "{} bytes", reply.len());
            let page = Page {
                items: vec![agent.clone()],
                next_cursor: None,
            };
            let agents = fmt.agents(f, &page);
            assert!(agents.len() <= cap, "{} bytes", agents.len());
            assert!(!agents.contains("larger than"), "{agents}");
            let members = fmt.members(
                f,
                &Members {
                    channel_id: "ch_7k2m9q4x".into(),
                    total: 1,
                    page,
                },
            );
            assert!(members.len() <= cap, "{} bytes", members.len());
            assert!(!members.contains("larger than"), "{members}");
            if f == Format::Json {
                // Cut only when the whole one could not fit.
                let too_big = serde_json::to_string(&long).unwrap().len() > cap;
                for out in [&agents, &members] {
                    let json: Value = serde_json::from_str(out).expect("valid JSON");
                    assert_eq!(json.to_string().contains('…'), too_big, "{out}");
                }
            }
        }
    }
}

#[test]
fn descriptions_are_whole_when_their_row_fits() {
    // 300 ASCII characters fit whole even at the smallest cap.
    let long = "x".repeat(Limits::DEFAULT.max_description_chars);
    let agent = described("amber-falcon", Some(&long));
    let page = Page {
        items: vec![agent.clone()],
        next_cursor: None,
    };
    let out = formatter(crate::config::MIN_RESPONSE_BYTES).agents(Format::Json, &page);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["agents"][0]["description"], long.as_str());
    // At the default cap, so do the longest four-byte descriptions.
    let long = "😀".repeat(Limits::DEFAULT.max_description_chars);
    let page = Page {
        items: vec![described("amber-falcon", Some(&long))],
        next_cursor: None,
    };
    let out = formatter(Limits::DEFAULT.max_response_bytes).agents(Format::Json, &page);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["agents"][0]["description"], long.as_str());
}

#[test]
fn the_set_description_reply_gives_the_length_not_the_text() {
    let agent = described("amber-falcon", Some("héllo"));
    let fmt = formatter(8000);
    assert_eq!(
        fmt.description(Format::Text, &agent),
        "name: amber-falcon\ndescription set (5 characters)"
    );
    let json: Value = serde_json::from_str(&fmt.description(Format::Json, &agent)).unwrap();
    assert_eq!(json["description_chars"], 5);
    let cleared = described("amber-falcon", None);
    assert_eq!(
        fmt.description(Format::Text, &cleared),
        "name: amber-falcon\ndescription cleared"
    );
}

#[test]
fn cut_bytes_keeps_characters_whole() {
    assert_eq!(cut_bytes("héllo", 5), "h…");
    assert_eq!(cut_bytes("héllo", 6), "héllo");
    assert_eq!(cut_bytes("😀😀", 7), "😀…");
}

#[test]
fn short_times() {
    let now = Utc.with_ymd_and_hms(2026, 10, 23, 15, 0, 0).unwrap();
    assert_eq!(short_time("2026-10-23T09:05:00Z", now), "09:05");
    assert_eq!(short_time("2026-10-22T09:05:00Z", now), "2026-10-22 09:05");
}

#[test]
fn cut_counts_characters() {
    assert_eq!(cut_body("héllo", 2), ("hé…".to_string(), true));
    assert_eq!(cut_body("hi", 2), ("hi".to_string(), false));
}
