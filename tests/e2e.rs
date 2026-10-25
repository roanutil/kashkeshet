//! End-to-end tests: start the server, then run agents through it with
//! rmcp's client over HTTP.

mod common;

use common::{Client, Server, connect, connect_new, start, start_with, test_config};
use kashkeshet::hosts::AllowedHosts;
use rmcp::ClientLifecycleMode;
use rmcp::model::{CallToolRequestParams, ProtocolVersion};
use serde_json::{Value, json};
use std::time::Duration;

/// Calls a tool. Returns the text and whether it was an error.
async fn call(client: &Client, tool: &str, args: Value) -> (String, bool) {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let result = client.call_tool(params).await.expect("call_tool");
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    (text, result.is_error == Some(true))
}

async fn ok(client: &Client, tool: &str, args: Value) -> String {
    let (text, is_error) = call(client, tool, args).await;
    assert!(!is_error, "{tool} failed: {text}");
    text
}

/// Registers and returns (name, token).
async fn register(client: &Client) -> (String, String) {
    let text = ok(client, "register", json!({})).await;
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}: ")))
            .unwrap_or_else(|| panic!("no {key} in {text}"))
            .to_string()
    };
    (field("name"), field("token"))
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

#[tokio::test]
async fn three_agents_talk() {
    let server = start().await;
    let (alice_c, bob_c, carol_c) = (
        connect_new(&server).await,
        connect_new(&server).await,
        connect_new(&server).await,
    );
    let (alice, at) = register(&alice_c).await;
    let (bob, bt) = register(&bob_c).await;
    let (carol, ct) = register(&carol_c).await;

    assert_eq!(
        ok(&bob_c, "whoami", json!({"token": bt})).await,
        format!("name: {bob}")
    );

    // Alice opens a public channel; Bob finds it by its purpose and joins.
    let created = ok(
        &alice_c,
        "create_channel",
        json!({"token": at, "name": "parser", "open": true, "purpose": "Fixing the TOML parser", "format": "json"}),
    )
    .await;
    let general = json_of(&created)["id"].as_str().unwrap().to_string();
    let found = ok(
        &bob_c,
        "list_channels",
        json!({"token": bt, "query": "toml"}),
    )
    .await;
    assert!(found.contains(&general), "{found}");
    ok(
        &bob_c,
        "join_channel",
        json!({"token": bt, "channel_id": general}),
    )
    .await;

    // Bob waits; Alice speaks; Bob hears it.
    let waiting = {
        let bt = bt.clone();
        tokio::spawn(async move {
            call(
                &bob_c,
                "wait_for_messages",
                json!({"token": bt, "after_id": 0, "timeout_s": 20, "format": "json"}),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let sent = ok(
        &alice_c,
        "send",
        json!({"token": at, "channel_id": general, "body": "can someone take the parser?"}),
    )
    .await;
    let first_id: i64 = sent
        .strip_prefix("sent: message_id=")
        .unwrap()
        .parse()
        .unwrap();
    let (heard, is_error) = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("wait should return")
        .expect("join");
    assert!(!is_error, "{heard}");
    let heard = json_of(&heard);
    assert_eq!(heard["messages"][0]["id"], first_id);
    assert_eq!(heard["messages"][0]["agent"], alice);

    // Alice and Carol talk privately. Bob cannot see the channel at all.
    let dm = ok(
        &alice_c,
        "create_channel",
        json!({"token": at, "name": "dm", "open": false, "members": [carol], "format": "json"}),
    )
    .await;
    let dm = json_of(&dm)["id"].as_str().unwrap().to_string();
    ok(
        &carol_c,
        "send",
        json!({"token": ct, "channel_id": dm, "body": "hi alice"}),
    )
    .await;
    // Bob's first client is busy in the finished wait task; identity lives in
    // the token, so a fresh connection works just as well.
    let bob_c = connect_new(&server).await;
    let (denied, is_error) = call(&bob_c, "read", json!({"token": bt, "channel_id": dm})).await;
    assert!(is_error);
    assert!(denied.starts_with("error: not_found:"), "{denied}");
    let bobs = ok(&bob_c, "list_channels", json!({"token": bt})).await;
    assert!(!bobs.contains(&dm));

    // Alice sees activity in both channels, then reads the DM.
    let activity = ok(
        &alice_c,
        "activity",
        json!({"token": at, "after_id": first_id}),
    )
    .await;
    assert!(activity.contains(&dm), "{activity}");
    let read = ok(&alice_c, "read", json!({"token": at, "channel_id": dm})).await;
    assert!(read.starts_with("#dm ["), "{read}");
    assert!(read.contains(&format!("{carol}] hi alice")), "{read}");

    // Search only covers your own channels.
    let hits = ok(
        &alice_c,
        "search_messages",
        json!({"token": at, "query": "parser"}),
    )
    .await;
    assert!(hits.contains(&general), "{hits}");
}

#[tokio::test]
async fn tokens_not_connections_carry_identity() {
    let server = start().await;
    let first = connect_new(&server).await;
    let (name, token) = register(&first).await;
    drop(first);
    let second = connect_new(&server).await;
    assert_eq!(
        ok(&second, "whoami", json!({"token": token})).await,
        format!("name: {name}")
    );

    let (text, is_error) = call(&second, "whoami", json!({})).await;
    assert!(is_error);
    assert!(
        text.contains("unauthenticated") && text.contains("register"),
        "{text}"
    );
}

#[tokio::test]
async fn older_clients_work_without_sessions() {
    let server = start().await;
    let client = connect(&server, ClientLifecycleMode::Initialize).await;
    let (_, token) = register(&client).await;
    let created = ok(
        &client,
        "create_channel",
        json!({"token": token, "name": "legacy", "open": true}),
    )
    .await;
    assert!(created.starts_with("#legacy ["), "{created}");
}

#[tokio::test]
async fn long_messages_are_cut_and_fetchable_in_full() {
    let server = start().await;
    let c = connect_new(&server).await;
    let (_, t) = register(&c).await;
    let ch = json_of(
        &ok(
            &c,
            "create_channel",
            json!({"token": t, "name": "x", "open": true, "format": "json"}),
        )
        .await,
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let body = "word ".repeat(300);
    let sent = ok(
        &c,
        "send",
        json!({"token": t, "channel_id": ch, "body": body}),
    )
    .await;
    let id = sent.strip_prefix("sent: message_id=").unwrap();
    let read = ok(&c, "read", json!({"token": t, "channel_id": ch})).await;
    assert!(read.contains(&format!("(cut; get_message {id})")), "{read}");
    let full = ok(
        &c,
        "get_message",
        json!({"token": t, "message_id": id.parse::<i64>().unwrap(), "format": "json"}),
    )
    .await;
    assert_eq!(json_of(&full)["body"], body);
}

#[tokio::test]
async fn posting_a_token_is_refused() {
    let server = start().await;
    let c = connect_new(&server).await;
    let (_, t) = register(&c).await;
    let ch = json_of(
        &ok(
            &c,
            "create_channel",
            json!({"token": t, "name": "x", "open": true, "format": "json"}),
        )
        .await,
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (text, is_error) = call(
        &c,
        "send",
        json!({"token": t, "channel_id": ch, "body": format!("mine is {t}")}),
    )
    .await;
    assert!(is_error);
    assert!(text.starts_with("error: contains_secret"), "{text}");
}

#[tokio::test]
async fn browser_origins_are_refused_unless_allowed() {
    let server = start().await;
    let http = reqwest::Client::new();
    let post = |origin: &'static str| {
        http.post(format!("{}/mcp", server.base))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "server/discover")
            .header("Origin", origin)
            .body(
                json!({
                    "jsonrpc": "2.0", "id": 1, "method": "server/discover",
                    "params": { "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities": {},
                    }},
                })
                .to_string(),
            )
            .send()
    };
    let evil = post("https://evil.example").await.expect("request");
    assert_eq!(evil.status(), reqwest::StatusCode::FORBIDDEN);
    let allowed = post("http://localhost:6274").await.expect("request");
    let status = allowed.status();
    assert!(
        status.is_success(),
        "{status}: {}",
        allowed.text().await.unwrap_or_default()
    );
}

/// Status codes for `/mcp` and the web page when a request names `host`.
async fn statuses_for_host(server: &Server, host: &str) -> (u16, u16) {
    let http = reqwest::Client::new();
    let port = server.base.rsplit(':').next().expect("port");
    let host = format!("{host}:{port}");
    let mcp = http
        .post(format!("{}/mcp", server.base))
        .header("Host", &host)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "server/discover",
                "params": { "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                }},
            })
            .to_string(),
        )
        .send()
        .await
        .expect("mcp request");
    let web = http
        .get(format!("{}/", server.base))
        .header("Host", &host)
        .send()
        .await
        .expect("web request");
    (mcp.status().as_u16(), web.status().as_u16())
}

#[tokio::test]
async fn other_host_names_are_refused_unless_allowed() {
    let server = start().await;
    assert_eq!(statuses_for_host(&server, "localhost").await, (200, 200));
    assert_eq!(
        statuses_for_host(&server, "host.docker.internal").await,
        (403, 403)
    );

    let mut config = test_config();
    config.allowed_hosts = AllowedHosts::new(&["host.docker.internal".into()]).expect("valid host");
    let server = start_with(config).await;
    assert_eq!(
        statuses_for_host(&server, "host.docker.internal").await,
        (200, 200)
    );
    assert_eq!(statuses_for_host(&server, "localhost").await, (200, 200));
    assert_eq!(statuses_for_host(&server, "evil.example").await, (403, 403));
}

#[tokio::test]
async fn web_pages_show_everything() {
    let server = start().await;
    let c = connect_new(&server).await;
    let (name, t) = register(&c).await;
    let ch = json_of(
        &ok(
            &c,
            "create_channel",
            json!({"token": t, "name": "hidden-room", "open": false, "format": "json"}),
        )
        .await,
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &c,
        "send",
        json!({"token": t, "channel_id": ch, "body": "<b>not bold</b>"}),
    )
    .await;

    let http = reqwest::Client::new();
    let get = |path: String| {
        let http = http.clone();
        let base = server.base.clone();
        async move {
            let r = http.get(format!("{base}{path}")).send().await.expect("get");
            (r.status(), r.text().await.expect("body"))
        }
    };
    let (status, index) = get("/".into()).await;
    assert!(status.is_success());
    assert!(index.contains("hidden-room") && index.contains(&name));
    // Bodies are escaped, not rendered as HTML.
    assert!(
        index.contains("&#60;b&#62;not bold") || index.contains("&lt;b&gt;not bold"),
        "{index}"
    );

    // The server run is listed, with the settings it ran with.
    assert!(index.contains("Server runs (1)"), "{index}");
    assert!(index.contains("default_limit=20"), "{index}");

    let (status, page) = get(format!("/channels/{ch}")).await;
    assert!(status.is_success());
    assert!(page.contains("not bold"));
    assert!(page.contains("· run 1"), "{page}");
    let (status, page) = get(format!("/agents/{name}")).await;
    assert!(status.is_success());
    assert!(page.contains("created in server run 1"), "{page}");
    let (status, _) = get("/channels/ch_missing".into()).await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn web_events_stream_new_messages() {
    let server = start().await;
    let c = connect_new(&server).await;
    let (_, t) = register(&c).await;
    let ch = json_of(
        &ok(
            &c,
            "create_channel",
            json!({"token": t, "name": "x", "open": true, "format": "json"}),
        )
        .await,
    )["id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut stream = reqwest::Client::new()
        .get(format!("{}/events", server.base))
        .send()
        .await
        .expect("events");
    tokio::time::sleep(Duration::from_millis(100)).await;
    ok(
        &c,
        "send",
        json!({"token": t, "channel_id": ch, "body": "live!"}),
    )
    .await;

    let mut seen = String::new();
    let found = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = stream.chunk().await.expect("chunk") {
            seen.push_str(&String::from_utf8_lossy(&chunk));
            if seen.contains("live!") {
                return true;
            }
        }
        false
    })
    .await
    .expect("event should arrive");
    assert!(found);
    assert!(seen.contains("event: message"), "{seen}");
}

#[tokio::test]
async fn agents_receive_the_configured_instructions() {
    let server = start().await;
    for lifecycle in [
        ClientLifecycleMode::Initialize,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    ] {
        let client = connect(&server, lifecycle).await;
        let info = client.peer_info().expect("server info");
        assert_eq!(
            info.instructions.as_deref(),
            Some("test instructions: call register")
        );
    }
}
