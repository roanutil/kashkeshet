//! Exercise actual OS signals against `kkt serve`.
#![cfg(unix)]

use rmcp::{ServiceExt, model::CallToolRequestParams, transport::StreamableHttpClientTransport};
use serde_json::json;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::home::OwnHome;

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub enum OpenRequest {
    None,
    Events,
    Wait,
}

pub async fn stops_cleanly(signal: &str, log_signal: &str, open: OpenRequest) {
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("free port")
        .local_addr()
        .expect("address");
    let base = format!("http://{addr}");
    let dir = std::env::temp_dir().join(format!(
        "kashkeshet-shutdown-{}-{}",
        std::process::id(),
        addr.port()
    ));
    std::fs::create_dir_all(&dir).expect("temporary database directory");
    // Its own home, so the server reads no real config file; no key, so the
    // requests below need none.
    let mut child = Command::new(env!("CARGO_BIN_EXE_kkt"))
        .arg("serve")
        .arg("--db")
        .arg(dir.join("server.db"))
        .args(["--bind", &addr.to_string(), "--no-access-key"])
        .own_home(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("start server");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let line = lines
                .next_line()
                .await
                .expect("read log")
                .expect("startup log");
            if line.contains("listening: web page") {
                break;
            }
        }
    })
    .await
    .expect("server should become ready");

    // Keep the response alive through shutdown: dropping it beforehand would
    // hide a server that waits forever for long-lived requests to finish.
    let http = reqwest::Client::new();
    let mut waiting = None;
    let response = tokio::time::timeout(TIMEOUT, async {
        match open {
            OpenRequest::None => None,
            OpenRequest::Events => Some(
                http.get(format!("{base}/events"))
                    .send()
                    .await
                    .expect("events"),
            ),
            OpenRequest::Wait => {
                let client = ()
                    .serve(StreamableHttpClientTransport::from_uri(format!(
                        "{base}/mcp"
                    )))
                    .await
                    .expect("MCP client");
                let registered = client
                    .call_tool(CallToolRequestParams::new("register"))
                    .await
                    .expect("register");
                assert_ne!(registered.is_error, Some(true), "registration failed");
                let text = &registered.content[0]
                    .as_text()
                    .expect("registration text")
                    .text;
                let token = text
                    .lines()
                    .find_map(|line| line.strip_prefix("token: "))
                    .expect("token");
                let params = CallToolRequestParams::new("wait_for_messages").with_arguments(
                    json!({"token":token,"after_id":0,"timeout_s":50})
                        .as_object()
                        .expect("arguments")
                        .clone(),
                );
                waiting = Some(tokio::spawn(async move {
                    let result = client.call_tool(params).await;
                    client.cancel().await.expect("close MCP client");
                    result
                }));
                None
            }
        }
    })
    .await
    .expect("open request before shutdown");
    if response.as_ref().is_some_and(|r| !r.status().is_success()) {
        let response = response.expect("response");
        panic!(
            "{}: {}",
            response.status(),
            response.text().await.expect("error body")
        );
    }
    if let Some(waiting) = &mut waiting {
        assert!(
            tokio::time::timeout(Duration::from_millis(100), waiting)
                .await
                .is_err(),
            "message wait must still be pending before shutdown"
        );
    }

    let sent = Command::new("kill")
        .args([signal, &child.id().expect("pid").to_string()])
        .status()
        .await
        .expect("send signal");
    assert!(sent.success());
    let status = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .expect("server should stop within timeout")
        .expect("wait for server");
    let mut log = String::new();
    while let Some(line) = lines.next_line().await.expect("read shutdown log") {
        log.push_str(&line);
        log.push('\n');
    }
    assert!(status.success(), "unclean exit: {status}; {log}");
    assert!(
        log.contains(&format!("received {log_signal}; shutting down")),
        "missing shutdown log: {log}"
    );
    if let Some(response) = response {
        let body = tokio::time::timeout(TIMEOUT, response.text())
            .await
            .expect("response should finish")
            .expect("clean response body end");
        assert!(!body.contains("\"isError\":true"), "{body}");
    }
    if let Some(waiting) = waiting {
        let result = tokio::time::timeout(TIMEOUT, waiting)
            .await
            .expect("MCP wait should end")
            .expect("wait task");
        if let Ok(result) = result {
            assert_ne!(result.is_error, Some(true), "wait returned a tool error");
        }
        // Transport cancellation may end the call before the empty result is sent.
    }
    std::fs::remove_dir_all(dir).expect("remove temporary database");
}
