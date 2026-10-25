//! The MCP tools are a contract with every agent that uses the server. This
//! test compares the tools the server offers (names, descriptions, and input
//! schemas) with the snapshot in `tests/contract/tools.json`, so any change to
//! what agents see shows up as a failure.
//!
//! After an intended change, rewrite the snapshot with
//! `UPDATE_CONTRACT=1 cargo test --test contract` and review the diff.

mod common;

use kashkeshet::config::Limits;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

const UPDATE_VAR: &str = "UPDATE_CONTRACT";

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/contract/tools.json")
}

#[tokio::test]
async fn tools_match_the_snapshot() {
    let server = common::start().await;
    let client = common::connect_new(&server).await;
    let mut tools = client.list_all_tools().await.expect("list tools");
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let actual = serde_json::to_value(&tools).expect("tools as JSON");
    let path = snapshot_path();

    if std::env::var_os(UPDATE_VAR).is_some() {
        let text = serde_json::to_string_pretty(&actual).expect("pretty JSON");
        std::fs::write(&path, text + "\n").expect("write snapshot");
        return;
    }

    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "could not read {}: {e}. Create it with {UPDATE_VAR}=1",
            path.display()
        )
    });
    let expected: Value = serde_json::from_str(&text).expect("snapshot is JSON");
    assert!(
        actual == expected,
        "the MCP tools changed. If that was intended, run \
         `{UPDATE_VAR}=1 cargo test --test contract` and review the diff.\nactual:\n{}",
        serde_json::to_string_pretty(&actual).expect("pretty JSON")
    );
}

/// The limits in tool descriptions come from the server's settings, not from
/// fixed text, so a server started with other limits states its own.
#[tokio::test]
async fn descriptions_state_the_live_limits() {
    let mut config = common::test_config();
    config.limits = Limits {
        default_limit: 7,
        max_limit: 42,
        max_body_chars: 99,
        max_body_bytes: 3000,
        max_channel_name_chars: 33,
        max_purpose_chars: 77,
        default_wait: Duration::from_secs(5),
        max_wait: Duration::from_secs(9),
        ..Limits::DEFAULT
    };
    let server = common::start_with(config).await;
    let client = common::connect_new(&server).await;
    let tools = client.list_all_tools().await.expect("list tools");
    let param = |tool: &str, name: &str| -> String {
        let tool = tools.iter().find(|t| t.name == tool).expect(tool);
        tool.input_schema["properties"][name]["description"]
            .as_str()
            .expect(name)
            .to_string()
    };

    assert_eq!(
        param("read", "limit"),
        "How many to return (default 7, up to 42)."
    );
    assert_eq!(
        param("read", "max_body_chars"),
        "Cut bodies longer than this many characters (default 99)."
    );
    assert_eq!(param("send", "body"), "The message text, up to 3000 bytes.");
    assert_eq!(
        param("create_channel", "name"),
        "A label for the channel, up to 33 characters. Need not be unique."
    );
    assert_eq!(
        param("set_purpose", "purpose"),
        "The new purpose, up to 77 characters. Empty clears it."
    );
    assert_eq!(
        param("wait_for_messages", "timeout_s"),
        "Seconds to wait (default 5, up to 9). 0 checks without waiting."
    );
}
