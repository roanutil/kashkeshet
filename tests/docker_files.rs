//! The Tailscale Compose setup: kkt and Tailscale are separate containers, and
//! Tailscale Serve proxies to the kkt service by name on the port kkt binds.
//! Only the Tailscale container may be on the tailnet, and kkt is published
//! only on the host's loopback.
//!
//! Both Compose files: the image version comes from `.env`, and the backups
//! folder is mounted, and kept out of git and image builds.

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

#[test]
fn both_compose_files_run_the_version_set_in_env_and_never_latest() {
    for file in ["compose.yaml", "compose.tailscale.yaml"] {
        let compose = read(file);
        let images: Vec<&str> = compose
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("image: ghcr.io/sam-mfb/kashkeshet:"))
            .collect();
        assert_eq!(
            images,
            ["${KKT_VERSION:?set KKT_VERSION in .env}"],
            "{file}"
        );
        assert!(!compose.contains("kashkeshet:latest"), "{file}");
    }
    assert!(read(".env.example").lines().any(|l| l == "KKT_VERSION="));
}

#[test]
fn both_compose_files_mount_the_backups_folder() {
    for file in ["compose.yaml", "compose.tailscale.yaml"] {
        assert!(
            read(file)
                .lines()
                .any(|l| l.trim() == "- ./backups:/backups"),
            "{file}"
        );
    }
}

#[test]
fn backups_never_go_into_git_or_an_image() {
    let gitignore = read(".gitignore");
    assert!(gitignore.lines().any(|l| l == "/backups/"));
    // No leading slash: an upgrade backup sits beside a database anywhere.
    assert!(gitignore.lines().any(|l| l == "upgrade-backups/"));
    let dockerignore = read(".dockerignore");
    assert!(dockerignore.lines().any(|l| l == "backups"));
    assert!(dockerignore.lines().any(|l| l == "**/upgrade-backups"));
}
