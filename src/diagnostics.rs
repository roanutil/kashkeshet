//! Read-only connection diagnostics. HTTP and JSON types stay in this part.

use crate::config::{self, DOCTOR_MAX_RESPONSE_BYTES, DOCTOR_PROTOCOL_VERSION, DOCTOR_TIMEOUT};
use reqwest::{Client, Response, Url, redirect::Policy};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
    pub fix: Option<String>,
}

impl Check {
    pub fn ok(name: &'static str, detail: impl Into<String>) -> Check {
        Check {
            name,
            ok: true,
            detail: detail.into(),
            fix: None,
        }
    }

    pub fn problem(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Check {
        Check {
            name,
            ok: false,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
}

/// Validate an HTTP MCP endpoint without allowing secrets in its URL. A bare
/// origin means /mcp; a supplied path is left intact for forwarding setups.
pub fn endpoint(bind: SocketAddr, given: Option<&str>) -> Result<String, String> {
    let default = format!("http://{bind}/mcp");
    let text = given.unwrap_or(&default);
    if text.chars().any(char::is_control) {
        return Err("URL contains a control character".into());
    }
    let mut url =
        Url::parse(text).map_err(|_| "URL must be an absolute http:// or https:// address")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("URL must be an absolute http:// or https:// address".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("put credentials in the key file, not in the URL; query strings and fragments are not supported".into());
    }
    if url.path() == "/" {
        url.set_path("/mcp");
    }
    Ok(url.to_string())
}

/// Check the operator key locally, without sending it to any endpoint.
pub fn inspect_web_key(path: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    key_file(Some(path), &mut checks);
    for check in &mut checks {
        check.name = match check.name {
            "key file" => "web key file",
            "key permissions" => "web key permissions",
            "key contents" => "web key contents",
            _ => "web key",
        };
        if check.fix.is_some() {
            check.fix = Some("check the operator web key on the server; `kkt key create --web` creates it if missing, then restart the server".into());
        }
    }
    checks
}

fn key_file(path: Option<&Path>, checks: &mut Vec<Check>) -> Option<String> {
    let Some(path) = path else {
        checks.push(Check::ok("key", "disabled in the serve config"));
        return None;
    };
    match std::fs::metadata(path) {
        Err(e) => {
            checks.push(Check::problem("key file", format!("{}: {e}", path.display()),
                "run `kkt serve` to create the default key, or pass --key-file with an existing key"));
            return None;
        }
        Ok(metadata) => {
            checks.push(Check::ok("key file", path.display().to_string()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode() & 0o777;
                if mode == config::ACCESS_KEY_FILE_MODE {
                    checks.push(Check::ok("key permissions", "0600"));
                } else {
                    checks.push(Check::problem(
                        "key permissions",
                        format!("{mode:04o}, expected 0600"),
                        format!(
                            "chmod 600 {}",
                            crate::clients::shell_word(&path.to_string_lossy())
                        ),
                    ));
                }
            }
        }
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match config::check_access_key(text.trim()) {
            Ok(()) => Some(text.trim().to_string()),
            Err(e) => {
                checks.push(Check::problem(
                    "key contents",
                    e.to_string(),
                    "choose a valid key file with --key-file",
                ));
                None
            }
        },
        Err(e) => {
            checks.push(Check::problem(
                "key contents",
                format!("could not read the key: {e}"),
                "make the key file readable by your user",
            ));
            None
        }
    }
}

fn initialized(text: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(text).ok()?;
    let candidates =
        std::iter::once(text).chain(text.lines().filter_map(|line| line.strip_prefix("data:")));
    candidates
        .filter_map(|candidate| serde_json::from_str::<Value>(candidate).ok())
        .find(|value| value.get("result").is_some() || value.get("error").is_some())
}

async fn initialize_response(mut response: Response) -> Result<Value, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("could not read MCP response: {e}"))?
    {
        if body.len() + chunk.len() > DOCTOR_MAX_RESPONSE_BYTES {
            return Err("MCP response exceeded the diagnostic size limit".into());
        }
        body.extend_from_slice(&chunk);
        if let Some(value) = initialized(&body) {
            return Ok(value);
        }
    }
    Err("the endpoint did not return an MCP initialize result".into())
}

/// Check a server without registering an agent or changing its database.
/// The caller adds the platform's service state using the service part.
/// The name of the check that the server answered at all.
pub const SERVER_CHECK: &str = "server";

/// Whether `checks` found the server not answering at all, as opposed to
/// answering with a problem.
pub fn unreachable(checks: &[Check]) -> bool {
    checks.iter().any(|c| c.name == SERVER_CHECK && !c.ok)
}

pub async fn inspect(url: &str, key_path: Option<&Path>) -> Result<Vec<Check>, String> {
    let mut checks = Vec::new();
    let key = key_file(key_path, &mut checks);
    probe(url, key.as_deref(), checks).await
}

/// The name of the check that the server accepted the key.
pub const KEY_CHECK: &str = "key accepted";

/// Checks that a server answers at `url`, accepts its host name, and serves
/// MCP there, without a key: a server that asks for one passes.
pub async fn inspect_address(url: &str) -> Result<Vec<Check>, String> {
    let mut checks = probe(url, None, Vec::new()).await?;
    checks.retain(|check| check.name != KEY_CHECK);
    Ok(checks)
}

/// Checks a server as [`inspect`] does, with the key given as text (`None`
/// for no key) rather than in a file.
pub async fn inspect_with_key(url: &str, key: Option<&str>) -> Result<Vec<Check>, String> {
    probe(url, key, Vec::new()).await
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(DOCTOR_TIMEOUT)
        .connect_timeout(DOCTOR_TIMEOUT)
        .redirect(Policy::none())
        .no_proxy()
        .https_only(false)
        .user_agent(concat!("kkt-doctor/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("could not construct the diagnostic HTTP client: {e}"))
}

/// Adds whether the server answers at `url` and accepts its host name to
/// `checks`. Returns whether both did.
async fn reach(client: &Client, url: &Url, checks: &mut Vec<Check>) -> bool {
    let response = match client.get(url.clone()).send().await {
        Ok(response) => response,
        Err(e) => {
            checks.push(Check::problem(
                SERVER_CHECK,
                e.to_string(),
                "run `kkt service start` or `kkt serve`, and check --url and the bind setting",
            ));
            return false;
        }
    };
    checks.push(Check::ok(SERVER_CHECK, format!("reachable at {url}")));
    if response.status().as_u16() == 403 {
        checks.push(Check::problem(
            "Host",
            "the server refused this host name",
            format!(
                "add {:?} to allow_hosts in the serve config, then restart",
                url.host_str().unwrap_or("")
            ),
        ));
        return false;
    }
    checks.push(Check::ok("Host", "accepted"));
    true
}

/// Adds the server checks for `url`, using `key`, to `checks`.
async fn probe(url: &str, key: Option<&str>, mut checks: Vec<Check>) -> Result<Vec<Check>, String> {
    let client = http_client()?;
    let url = Url::parse(url).map_err(|_| "invalid diagnostic URL")?;
    if !reach(&client, &url, &mut checks).await {
        return Ok(checks);
    }
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
        "protocolVersion": DOCTOR_PROTOCOL_VERSION,
        "capabilities": {}, "clientInfo":{"name":"kkt-doctor", "version":env!("CARGO_PKG_VERSION")}
    }});
    let mut post = client
        .post(url.clone())
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body(request.to_string());
    if let Some(key) = key {
        post = post.bearer_auth(key);
    }
    let response = match post.send().await {
        Ok(response) => response,
        Err(e) => {
            checks.push(Check::problem(
                "MCP",
                e.to_string(),
                "check the MCP URL and server logs",
            ));
            return Ok(checks);
        }
    };
    if response.status().as_u16() == 401 {
        checks.push(Check::problem(KEY_CHECK, "server returned 401", "pass --key-file with the key used by the running server, or restart it after changing keys"));
        return Ok(checks);
    }
    if response.status().as_u16() != 200 {
        checks.push(Check::problem(
            "MCP",
            format!("server returned {}", response.status()),
            "check that --url points at the server's /mcp endpoint",
        ));
        return Ok(checks);
    }
    let session = response.headers().get("mcp-session-id").cloned();
    let initialized = initialize_response(response).await;
    if let Some(session) = session {
        let mut delete = client
            .delete(url)
            .header("mcp-session-id", session)
            .header("MCP-Protocol-Version", DOCTOR_PROTOCOL_VERSION);
        if let Some(key) = key {
            delete = delete.bearer_auth(key);
        }
        match delete.send().await {
            Ok(response) if response.status().is_success() => (),
            Ok(response) => checks.push(Check::problem(
                "probe cleanup",
                format!("server returned {}", response.status()),
                "check the server logs",
            )),
            Err(e) => checks.push(Check::problem(
                "probe cleanup",
                e.to_string(),
                "check the server logs",
            )),
        }
    }
    match initialized {
        Ok(value) => match value.get("result").and_then(|r| r.get("serverInfo")) {
            Some(info) => {
                checks.push(Check::ok(KEY_CHECK, "MCP initialize returned 200"));
                let version = info
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("not reported");
                checks.push(Check::ok("server build", format!("version {version}; commit is not exposed by MCP (see the web page's server runs)")));
            }
            None => checks.push(Check::problem(
                "MCP",
                "initialize did not return serverInfo",
                "check --url and the server's MCP implementation",
            )),
        },
        Err(error) => checks.push(Check::problem(
            "MCP",
            error,
            "check --url and the server logs",
        )),
    }
    Ok(checks)
}
