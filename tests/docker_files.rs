//! The Tailscale Compose setup: kkt's loopback address must match the one
//! Tailscale Serve proxies to, and must not be 127.0.0.1, which tailnet
//! connections for the node's own address reach in userspace mode.

use std::net::SocketAddr;

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The address after `--bind` in the Compose file's kkt command.
fn compose_bind() -> SocketAddr {
    let compose = read("compose.tailscale.yaml");
    let mut lines = compose.lines().map(str::trim);
    lines
        .find(|line| *line == "- --bind")
        .expect("compose.tailscale.yaml passes --bind");
    let value = lines
        .next()
        .and_then(|line| line.strip_prefix("- "))
        .expect("--bind has a value");
    value.parse().expect("bind is an address")
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
fn serve_proxies_to_the_address_kkt_binds() {
    assert_eq!(serve_proxy(), format!("http://{}", compose_bind()));
}

#[test]
fn kkt_is_not_on_the_address_tailnet_connections_reach() {
    let bind = compose_bind();
    assert!(bind.ip().is_loopback(), "{bind}");
    assert_ne!(bind.ip().to_string(), "127.0.0.1", "{bind}");
}
