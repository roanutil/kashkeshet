//! The Tailscale Compose setup: kkt and Tailscale are separate containers, and
//! Tailscale Serve proxies to the kkt service by name on the port kkt binds.
//! Only the Tailscale container may be on the tailnet, and kkt is published
//! only on the host's loopback.

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The address after `--bind` in the Compose file's kkt command.
fn compose_bind() -> String {
    let compose = read("compose.tailscale.yaml");
    let mut lines = compose.lines().map(str::trim);
    lines
        .find(|line| *line == "- --bind")
        .expect("compose.tailscale.yaml passes --bind");
    lines
        .next()
        .and_then(|line| line.strip_prefix("- "))
        .expect("--bind has a value")
        .to_string()
}

fn serve_proxy() -> String {
    let serve: serde_json::Value =
        serde_json::from_str(&read("tailscale/serve.json")).expect("serve.json is JSON");
    serve["Web"]
        .as_object()
        .and_then(|web| web.values().next())
        .and_then(|site| site["Handlers"]["/"]["Proxy"].as_str())
        .expect("serve.json proxies /")
        .to_string()
}

#[test]
fn serve_proxies_to_the_kkt_service_on_the_port_kkt_binds() {
    let port = compose_bind()
        .rsplit(':')
        .next()
        .expect("bind has a port")
        .to_string();
    assert_eq!(serve_proxy(), format!("http://kkt:{port}"));
}

#[test]
fn kkt_listens_beyond_loopback_with_the_flag_that_allows_it() {
    let compose = read("compose.tailscale.yaml");
    assert!(compose_bind().starts_with("0.0.0.0:"));
    assert!(compose.contains("- --allow-any-bind"));
}

#[test]
fn kkt_does_not_share_the_tailscale_network() {
    assert!(!read("compose.tailscale.yaml").contains("network_mode"));
}

#[test]
fn kkt_is_published_on_the_host_loopback_only() {
    let compose = read("compose.tailscale.yaml");
    let published: Vec<&str> = compose
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("- \"") && line.matches(':').count() == 2)
        .collect();
    assert_eq!(published, ["- \"127.0.0.1:7420:7420\""]);
}
