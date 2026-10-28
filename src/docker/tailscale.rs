//! Reading `tailscale status --json` from the Tailscale container.

use super::DockerError;
use serde_json::Value;

/// What setup needs to know about the node.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Node {
    /// `Running` once logged in, `NeedsLogin` while waiting for a login.
    pub backend_state: String,
    /// The link to log in with, while one is needed.
    pub auth_url: Option<String>,
    /// The node's full name on the tailnet, without the trailing dot.
    pub dns_name: Option<String>,
    /// The names HTTPS certificates can be issued for. Empty when the tailnet
    /// has not enabled HTTPS.
    pub cert_domains: Vec<String>,
}

pub(super) fn parse_status(text: &str) -> Result<Node, DockerError> {
    let unexpected = |detail: String| DockerError::Unexpected {
        what: "`tailscale status --json`",
        detail,
    };
    let json: Value = serde_json::from_str(text).map_err(|e| unexpected(e.to_string()))?;
    let backend_state = json["BackendState"]
        .as_str()
        .ok_or_else(|| unexpected("no BackendState".into()))?
        .to_string();
    let non_empty = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Ok(Node {
        backend_state,
        auth_url: non_empty(&json["AuthURL"]),
        dns_name: non_empty(&json["Self"]["DNSName"]).map(|n| n.trim_end_matches('.').to_string()),
        cert_domains: json["CertDomains"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// A node's full name split into its own name and the tailnet's, such as
/// `kkt-1.tail1234.ts.net` into `kkt-1` and `tail1234.ts.net`.
pub(super) fn split_name(full: &str) -> Option<(String, String)> {
    let (hostname, tailnet) = full.split_once('.')?;
    if hostname.is_empty() || tailnet.is_empty() {
        return None;
    }
    Some((hostname.to_string(), tailnet.to_string()))
}
