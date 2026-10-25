//! The MCP tools are a contract with every agent that uses the server. This
//! test compares the tools the server offers (names, descriptions, and input
//! schemas) with the snapshot in `tests/contract/tools.json`, so any change to
//! what agents see shows up as a failure.
//!
//! After an intended change, rewrite the snapshot with
//! `UPDATE_CONTRACT=1 cargo test --test contract` and review the diff.

mod common;

use serde_json::Value;
use std::path::PathBuf;

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
