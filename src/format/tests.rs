//! Output tests: message formatting, the size cap, and the shared helpers.

use super::fit::{cut_body, cut_bytes, short_time};
use super::*;
use crate::clock::testing::FixedClock;
use crate::config::Limits;
use crate::rules::{AgentInfo, Members, Message, MessagePage, Page, Position, WaitResult};
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
    let out = formatter(8192).read(Format::Text, &page, Position::Latest, Some(500));
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
    let out = formatter(2000).read(Format::Text, &page, Position::Latest, None);
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
    let out = formatter(2000).read(Format::Json, &page, Position::After(0), None);
    assert!(out.len() <= 2000);
    let v: Value = serde_json::from_str(&out).unwrap();
    let shown = v["messages"].as_array().unwrap();
    let last = shown.last().unwrap()["id"].as_i64().unwrap();
    assert_eq!(shown[0]["id"], 1);
    assert_eq!(v["more"], format!("after_id={last}"));
}

#[test]
fn a_message_too_big_for_the_cap_is_shortened_to_fit() {
    let page = MessagePage {
        messages: vec![msg(9, &"z".repeat(5000))],
        more: None,
    };
    for requested in [None, Some(4000)] {
        let out = formatter(1000).read(Format::Text, &page, Position::Latest, requested);
        assert!(out.len() <= 1000);
        assert!(out.contains("zzzzzzzzzz"));
        assert!(out.contains("(cut; get_message 9)"));
        let out = formatter(1000).read(Format::Json, &page, Position::Latest, requested);
        assert!(out.len() <= 1000);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["messages"][0]["id"], 9);
        assert_eq!(v["messages"][0]["cut"], true);
    }
}

#[test]
fn a_waited_message_too_big_for_the_cap_is_shortened_to_fit() {
    let waited = WaitResult {
        messages: vec![msg(9, &"z".repeat(5000)), msg(10, "next")],
        more_waiting: 0,
    };
    for f in [Format::Text, Format::Json] {
        let out = formatter(1000).waited(f, &waited, None);
        assert!(out.len() <= 1000);
        assert!(out.contains("zzzzzzzzzz"));
        assert!(out.contains("after_id=9"));
    }
}

#[test]
fn an_explicit_body_cut_wins_when_it_is_shorter_than_the_cap_allows() {
    let page = MessagePage {
        messages: vec![msg(9, &"z".repeat(5000))],
        more: None,
    };
    let out = formatter(1000).read(Format::Text, &page, Position::Latest, Some(10));
    assert!(out.contains("zzzzzzzzzz…  (cut; get_message 9)"));
    assert!(!out.contains("zzzzzzzzzzz"));
}

#[test]
fn lists_cut_at_cap_with_cursor() {
    let page = Page {
        items: (0..100)
            .map(|i| AgentInfo {
                name: format!("agent-{i:03}"),
                created_at: "2026-10-23T14:00:00Z".into(),
                last_seen: None,
                listening: false,
                waiting_channels: Vec::new(),
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
        format!("more: cursor={}", last_shown.split(';').next().unwrap())
    );
}

fn described(name: &str, description: Option<&str>) -> AgentInfo {
    AgentInfo {
        name: name.into(),
        created_at: "2026-10-23T14:00:00Z".into(),
        last_seen: Some("2026-10-23T14:05:00Z".into()),
        listening: false,
        waiting_channels: Vec::new(),
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
        "amber-falcon  last call 14:05; not listening — Codex, reviews only\nbrave-beaver  last call 14:05; not listening"
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
        "ch_7k2m9q4x: 2 members\namber-falcon; not listening; last call 2026-10-23T14:05:00Z — Codex, reviews only\nbrave-beaver; not listening; last call 2026-10-23T14:05:00Z"
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
    assert_eq!(cut_body("héllo", Some(2)), ("hé…".to_string(), true));
    assert_eq!(cut_body("hi", Some(2)), ("hi".to_string(), false));
}

#[test]
fn bodies_are_whole_when_the_response_fits() {
    let body = "x".repeat(3000);
    let page = MessagePage {
        messages: vec![msg(5, &body)],
        more: None,
    };
    let out = formatter(8192).read(Format::Text, &page, Position::Latest, None);
    assert!(out.contains(&body));
    assert!(!out.contains("(cut;"));
}

#[test]
fn a_requested_body_cut_still_applies() {
    let page = MessagePage {
        messages: vec![msg(5, &"x".repeat(3000))],
        more: None,
    };
    let out = formatter(8192).read(Format::Text, &page, Position::Latest, Some(10));
    assert!(out.contains("xxxxxxxxxx…  (cut; get_message 5)"));
}

#[test]
fn cut_body_with_no_limit_is_whole() {
    assert_eq!(cut_body("hello", None), ("hello".to_string(), false));
}

#[test]
fn pinned_notes_are_lossless_and_paginated_after_json_escaping() {
    let fmt = formatter(1024);
    for text in [
        "hello\nworld".to_string(),
        "😀\n\t\"\\\0".repeat(1500),
        String::new(),
    ] {
        let note = crate::rules::ChannelNote {
            channel_id: "ch_notes".into(),
            text: text.clone(),
            edited_by: Some("amber-falcon".into()),
            edited_at: Some("2026-10-23T14:00:00Z".into()),
        };
        let mut offset = 0;
        let mut recovered = String::new();
        loop {
            let out = fmt.note(Format::Json, &note, offset);
            assert!(out.len() <= 1024);
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["edited_by"], "amber-falcon");
            assert_eq!(v["edited_at"], "2026-10-23T14:00:00Z");
            assert_eq!(v["offset"], offset);
            recovered.push_str(v["text"].as_str().unwrap());
            let Some(next) = v["next_offset"].as_u64() else {
                break;
            };
            assert!(next as usize > offset);
            offset = next as usize;
        }
        assert_eq!(recovered, text);
        let out = fmt.note(Format::Text, &note, 0);
        assert!(out.len() <= 1024);
        assert!(out.contains("last edited by amber-falcon at 2026-10-23T14:00:00Z"));
        if text.len() > 1024 {
            assert!(out.contains("next_offset:"));
        } else {
            assert!(out.ends_with(&text));
        }
    }
}

#[test]
fn channel_lists_preview_notes_and_show_edit_attribution() {
    let note = crate::rules::ChannelNote {
        channel_id: "ch_notes".into(),
        text: "A".repeat(1000),
        edited_by: Some("amber-falcon".into()),
        edited_at: Some("2026-10-23T14:00:00Z".into()),
    };
    let ch = crate::rules::ChannelInfo {
        id: "ch_notes".into(),
        name: "notes".into(),
        purpose: None,
        note,
        open: true,
        created_by: "amber-falcon".into(),
        created_at: "2026-10-23T14:00:00Z".into(),
        member_count: 1,
        members_preview: vec!["amber-falcon".into()],
        is_member: true,
        new_messages: None,
    };
    let fmt = formatter(1024);
    let page = Page {
        items: vec![ch],
        next_cursor: None,
    };
    let out = fmt.channels(Format::Text, &page, 120);
    assert!(out.contains("note: "));
    assert!(out.contains("amber-falcon at 2026-10-23T14:00:00Z"));
    assert!(!out.contains(&"A".repeat(121)));
    let out = fmt.channels(Format::Json, &page, 120);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["channels"][0]["note"], format!("{}…", "A".repeat(120)));
    assert_eq!(v["channels"][0]["note_cut"], true);
    assert_eq!(v["channels"][0]["note_edited_by"], "amber-falcon");
    assert_eq!(v["channels"][0]["note_edited_at"], "2026-10-23T14:00:00Z");
}

#[test]
fn escaped_note_previews_do_not_hide_a_channel_at_the_minimum_cap() {
    let note = crate::rules::ChannelNote {
        channel_id: "ch_notes".into(),
        text: "\0".repeat(1000),
        edited_by: Some("amber-falcon".into()),
        edited_at: Some("2026-10-23T14:00:00Z".into()),
    };
    let ch = crate::rules::ChannelInfo {
        id: "ch_notes".into(),
        name: "a".repeat(64),
        purpose: Some("p".repeat(120)),
        note,
        open: true,
        created_by: "amber-falcon".into(),
        created_at: "2026-10-23T14:00:00Z".into(),
        member_count: 3,
        members_preview: vec![
            "amber-falcon".into(),
            "silver-ferret".into(),
            "brave-beaver".into(),
        ],
        is_member: true,
        new_messages: None,
    };
    let page = Page {
        items: vec![ch],
        next_cursor: None,
    };
    let out = formatter(1024).channels(Format::Json, &page, 120);
    assert!(out.len() <= 1024);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["channels"].as_array().unwrap().len(), 1);
    assert_eq!(v["channels"][0]["note_cut"], true);
    assert!(v["channels"][0]["note"].as_str().unwrap().starts_with('\0'));
}

#[test]
fn many_waiting_channels_keep_paginated_agents_and_members_visible() {
    let fmt = formatter(crate::config::MIN_RESPONSE_BYTES);
    for description in ["reviewer".into(), "😀".repeat(300), "\u{1}\n".repeat(150)] {
        let mut a = described("amber-falcon", Some(&description));
        a.listening = true;
        a.waiting_channels = (0..500).map(|i| format!("ch_{i:08}")).collect();
        let members = Members {
            channel_id: "ch_public".into(),
            total: 2,
            page: Page {
                items: vec![a],
                next_cursor: Some("amber-falcon".into()),
            },
        };
        for format in [Format::Text, Format::Json] {
            for out in [
                fmt.agents(format, &members.page),
                fmt.members(format, &members),
            ] {
                assert!(out.len() <= crate::config::MIN_RESPONSE_BYTES);
                assert!(!out.contains("larger than"), "{out}");
                if matches!(format, Format::Json) {
                    let json: Value = serde_json::from_str(&out).unwrap();
                    let key = if json.get("agents").is_some() {
                        "agents"
                    } else {
                        "members"
                    };
                    assert_eq!(json[key].as_array().unwrap().len(), 1);
                    assert_eq!(json[key][0]["name"], "amber-falcon");
                    assert!(json[key][0]["waiting_channels_more"].as_u64().unwrap() > 0);
                    assert_eq!(json[key][0]["listening"], true);
                    assert_eq!(json["more"], "cursor=amber-falcon");
                } else {
                    assert!(out.contains("amber-falcon"));
                    assert!(out.contains("more waiting channels"));
                    assert!(out.ends_with("more: cursor=amber-falcon"));
                }
            }
        }
    }
}

#[test]
fn a_long_handoff_note_reads_back_whole_in_parts_within_the_cap() {
    let cap = crate::config::MIN_RESPONSE_BYTES;
    let text = "😀 \"quoted\"\nline ".repeat(200);
    let note = crate::rules::HandoffNote {
        name: "amber-falcon".into(),
        text: text.clone(),
        set_at: Some("2026-10-23T14:00:00Z".into()),
    };
    let fmt = formatter(cap);
    for f in [Format::Text, Format::Json] {
        let mut read = String::new();
        let mut offset = 0;
        let mut parts = 0;
        loop {
            let out = fmt.handoff_note(f, &note, offset);
            assert!(out.len() <= cap, "{} bytes", out.len());
            parts += 1;
            let next = match f {
                Format::Json => {
                    let json: Value = serde_json::from_str(&out).expect("valid JSON");
                    read.push_str(json["text"].as_str().expect("text"));
                    json["next_offset"].as_u64().map(|n| n as usize)
                }
                Format::Text => {
                    let (_, rest) = out.split_once("offset: ").expect("offset line");
                    let (_, body) = rest.split_once('\n').expect("text");
                    match body.rsplit_once("\nnext_offset: ") {
                        Some((part, tail)) => {
                            read.push_str(part);
                            tail.split(' ').next().and_then(|n| n.parse().ok())
                        }
                        None => {
                            read.push_str(body);
                            None
                        }
                    }
                }
            };
            match next {
                Some(n) => offset = n,
                None => break,
            }
        }
        assert!(parts > 1, "the note needed several parts");
        assert_eq!(read, text, "nothing is lost");
    }
}

#[test]
fn an_unset_handoff_note_says_so() {
    let note = crate::rules::HandoffNote {
        name: "amber-falcon".into(),
        text: String::new(),
        set_at: None,
    };
    assert_eq!(
        formatter(8000).handoff_note(Format::Text, &note, 0),
        "handoff note for amber-falcon\nset at (never)\noffset: 0\n"
    );
}
