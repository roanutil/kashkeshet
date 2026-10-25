mod common;

use kashkeshet::admission::{Admission, SharedKey};
use kashkeshet::diagnostics;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const KEY: &str = "0123456789abcdef0123456789abcdef";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Key(PathBuf);
impl Key {
    fn new(text: &str) -> Key {
        let path = std::env::temp_dir().join(format!(
            "kkt-doctor-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, text).expect("key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode");
        }
        Key(path)
    }
}
impl Drop for Key {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).expect("cleanup");
    }
}

async fn keyed() -> common::Server {
    let mut config = common::test_config();
    config.admission = Admission::SharedKey(SharedKey::new(KEY, "test").expect("key"));
    common::start_with(config).await
}

#[tokio::test]
async fn doctor_initializes_and_closes_a_real_mcp_session_without_registering() {
    let server = keyed().await;
    let key = Key::new(&format!("{KEY}\n"));
    let checks = diagnostics::inspect(&format!("{}/mcp", server.base), Some(&key.0))
        .await
        .expect("inspect");
    assert!(checks.iter().all(|c| c.ok), "{checks:?}");
    assert!(
        checks
            .iter()
            .any(|c| c.name == "server build" && c.detail.contains(env!("CARGO_PKG_VERSION")))
    );
    assert!(!format!("{checks:?}").contains(KEY));
    let page = reqwest::get(format!("{}/", server.base))
        .await
        .expect("page");
    assert!(page.status().is_success());
}

#[tokio::test]
async fn doctor_reports_wrong_keys_and_does_not_expose_them() {
    let server = keyed().await;
    let key = Key::new("wrong0123456789abcdef0123456789abcdef");
    let checks = diagnostics::inspect(&format!("{}/mcp", server.base), Some(&key.0))
        .await
        .expect("inspect");
    assert!(
        checks.iter().any(|c| c.name == "key accepted"
            && !c.ok
            && c.detail.contains("401")
            && c.fix.is_some())
    );
    assert!(!format!("{checks:?}").contains("wrong0123456789"));
}

#[tokio::test]
async fn missing_key_is_reported_without_creating_it() {
    let server = keyed().await;
    let key = Key::new(KEY);
    let missing = key.0.with_extension("missing");
    let checks = diagnostics::inspect(&format!("{}/mcp", server.base), Some(&missing))
        .await
        .expect("inspect");
    assert!(checks.iter().any(|c| c.name == "key file" && !c.ok));
    assert!(!missing.exists());
}

#[tokio::test]
async fn explicit_no_key_mode_checks_an_open_server() {
    let server = common::start().await;
    let checks = diagnostics::inspect(&format!("{}/mcp", server.base), None)
        .await
        .expect("inspect");
    assert!(checks.iter().all(|c| c.ok), "{checks:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_world_readable_key_is_reported_even_if_authentication_works() {
    use std::os::unix::fs::PermissionsExt;
    let server = keyed().await;
    let key = Key::new(KEY);
    std::fs::set_permissions(&key.0, std::fs::Permissions::from_mode(0o644)).expect("mode");
    let checks = diagnostics::inspect(&format!("{}/mcp", server.base), Some(&key.0))
        .await
        .expect("inspect");
    assert!(checks.iter().any(|c| {
        c.name == "key permissions"
            && !c.ok
            && c.fix
                .as_deref()
                .is_some_and(|fix| fix.contains("chmod 600"))
    }));
    assert!(checks.iter().any(|c| c.name == "key accepted" && c.ok));
}

#[tokio::test]
async fn host_refusal_has_a_distinct_fix() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().fallback(|| async { axum::http::StatusCode::FORBIDDEN }),
        )
        .await
        .expect("serve");
    });
    let checks = diagnostics::inspect(&format!("http://{address}/mcp"), None)
        .await
        .expect("inspect");
    task.abort();
    assert!(checks.iter().any(|c| {
        c.name == "Host"
            && !c.ok
            && c.fix
                .as_deref()
                .is_some_and(|fix| fix.contains("allow_hosts"))
    }));
    assert!(!checks.iter().any(|c| c.name == "key accepted"));
}

#[tokio::test]
async fn unreachable_server_reports_how_to_start_it() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
    let address = listener.local_addr().expect("address");
    drop(listener);
    let checks = diagnostics::inspect(&format!("http://{address}/mcp"), None)
        .await
        .expect("inspect");
    assert!(checks.iter().any(|c| {
        c.name == "server"
            && !c.ok
            && c.fix
                .as_deref()
                .is_some_and(|fix| fix.contains("kkt serve"))
    }));
}

#[test]
fn endpoint_validation_keeps_keys_out_of_urls_and_preserves_forwarding_paths() {
    let bind = "[::1]:7420".parse().expect("bind");
    assert_eq!(
        diagnostics::endpoint(bind, None).expect("default"),
        "http://[::1]:7420/mcp"
    );
    assert_eq!(
        diagnostics::endpoint(bind, Some("https://tail.example/prefix/mcp")).expect("url"),
        "https://tail.example/prefix/mcp"
    );
    for bad in [
        "file:///tmp/key",
        "http://name:secret@localhost/mcp",
        "http://localhost/mcp?key=secret",
        "http://localhost/mcp#secret",
        "http://localhost/\nmcp",
    ] {
        let error = diagnostics::endpoint(bind, Some(bad)).expect_err("refused");
        assert!(!error.contains("secret"));
    }
}
