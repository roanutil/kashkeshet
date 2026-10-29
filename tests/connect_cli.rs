//! CLI integration uses an isolated HOME and a fake Claude executable.

mod common;

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const KEY: &str = "0123456789abcdef0123456789abcdef";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Home(PathBuf);
impl Home {
    fn new() -> Home {
        let path = std::env::temp_dir().join(format!(
            "kkt-connect-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("home");
        Home(path)
    }
    fn put(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        std::fs::write(&path, text).expect("file");
        path
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kkt"));
        command
            .env("HOME", &self.0)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .current_dir(&self.0);
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("run kkt")
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("cleanup");
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn connecting_creates_the_default_key_but_preview_does_not_write_client_config() {
    let home = Home::new();
    let out = home.run(&["connect", "codex"]);
    assert!(out.status.success(), "{}", text(&out));
    let key_file = home.0.join(".config/kkt/access-key");
    let key = std::fs::read_to_string(&key_file).expect("created key");
    assert!(!text(&out).contains(key.trim()));
    assert!(!home.0.join(".codex/config.toml").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&key_file)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let written = home.run(&["connect", "codex", "--write"]);
    assert!(written.status.success(), "{}", text(&written));
    let config_file = home.0.join(".codex/config.toml");
    let config = std::fs::read_to_string(&config_file).expect("config");
    let parsed: toml::Value = toml::from_str(&config).expect("TOML");
    let helper = parsed["mcp_servers"]["kkt"]["http_headers_helper"]
        .as_str()
        .expect("helper");
    assert!(helper.starts_with('\''));
    let result = Command::new("sh")
        .args(["-c", helper])
        .env("HOME", &home.0)
        .output()
        .expect("helper");
    assert!(result.status.success(), "{}", text(&result));
    let headers: serde_json::Value = serde_json::from_slice(&result.stdout).expect("helper JSON");
    assert_eq!(headers["Authorization"], format!("Bearer {}", key.trim()));
    let again = home.run(&["connect", "codex", "--write"]);
    assert!(again.status.success(), "{}", text(&again));
    assert_eq!(
        std::fs::read_to_string(&config_file).expect("config"),
        config
    );
    assert!(!config_file.with_file_name("config.toml.bak").exists());
}

#[test]
fn connect_muse_code_writes_settings_with_schema_version_and_is_idempotent() {
    let home = Home::new();
    let key = home.put("key", KEY);
    let key_arg = key.to_str().expect("key path");
    let preview = home.run(&["connect", "muse-code", "--key-file", key_arg]);
    assert!(preview.status.success(), "{}", text(&preview));
    assert!(
        text(&preview).contains(".config/muse/settings.json"),
        "{}",
        text(&preview)
    );
    assert!(
        text(&preview).contains("Muse Code stores the key"),
        "{}",
        text(&preview)
    );
    assert!(!text(&preview).contains(KEY));
    assert!(!home.0.join(".config/muse/settings.json").exists());

    let written = home.run(&["connect", "muse-code", "--key-file", key_arg, "--write"]);
    assert!(written.status.success(), "{}", text(&written));
    assert!(
        text(&written).contains("Muse Code stores the key"),
        "{}",
        text(&written)
    );
    assert!(!text(&written).contains(KEY));
    let file = home.0.join(".config/muse/settings.json");
    let config = std::fs::read_to_string(&file).expect("config");
    let parsed: serde_json::Value = serde_json::from_str(&config).expect("JSON");
    assert_eq!(parsed["schema_version"], 1);
    let entry = &parsed["mcpServers"]["kkt"];
    assert_eq!(entry["transport"], "streamable_http");
    assert!(entry["url"].as_str().expect("url").ends_with("/mcp"));
    assert_eq!(entry["mode"], "optional");
    assert_eq!(entry["headers"]["Authorization"], format!("Bearer {KEY}"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).expect("mode").permissions().mode() & 0o777,
            0o600
        );
    }
    let again = home.run(&["connect", "muse-code", "--key-file", key_arg, "--write"]);
    assert!(again.status.success(), "{}", text(&again));
    assert!(
        text(&again).contains("Already configured"),
        "{}",
        text(&again)
    );
    assert_eq!(std::fs::read_to_string(&file).expect("config"), config);
    assert!(!home.0.join(".config/muse/settings.json.bak").exists());
}

#[test]
fn key_header_reads_paths_with_quotes_and_outputs_only_requested_format() {
    let home = Home::new();
    let key = home.put("key with ' quote", &format!("  {KEY}\n"));
    let path = key.to_str().expect("path");
    let out = home.run(&["key", "header", "--key-file", path]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        String::from_utf8(out.stdout).expect("stdout"),
        format!("Authorization: Bearer {KEY}\n")
    );
    let written = home.run(&["connect", "codex", "--key-file", path, "--write"]);
    assert!(written.status.success(), "{}", text(&written));
    let config: toml::Value = toml::from_str(
        &std::fs::read_to_string(home.0.join(".codex/config.toml")).expect("config"),
    )
    .expect("TOML");
    let helper = config["mcp_servers"]["kkt"]["http_headers_helper"]
        .as_str()
        .expect("helper");
    let out = Command::new("sh")
        .args(["-c", helper])
        .env("HOME", &home.0)
        .output()
        .expect("helper");
    assert!(out.status.success(), "{}", text(&out));
    let headers: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(headers["Authorization"], format!("Bearer {KEY}"));
}

#[test]
fn client_defaults_follow_serve_config_and_explicit_overrides_win() {
    let home = Home::new();
    home.put(
        ".config/kkt/config.toml",
        "bind = '127.0.0.1:9876'\naccess_key_file = 'custom-key'\n",
    );
    home.put(".config/kkt/custom-key", KEY);
    let out = home.run(&["connect", "codex"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("http://127.0.0.1:9876/mcp"));
    assert!(text(&out).contains("custom-key"));
    assert!(!home.0.join(".config/kkt/access-key").exists());
    let out = home.run(&["connect", "opencode", "--url", "https://tail.example/mcp"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("https://tail.example/mcp"));
    assert!(!text(&out).contains(KEY));
}

#[test]
fn no_key_config_does_not_create_or_configure_credentials() {
    let home = Home::new();
    home.put(".config/kkt/config.toml", "no_access_key = true\n");
    let out = home.run(&["connect", "opencode", "--write"]);
    assert!(out.status.success(), "{}", text(&out));
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.0.join(".config/opencode/opencode.json")).expect("config"),
    )
    .expect("JSON");
    assert!(config["mcp"]["kkt"].get("headers").is_none());
    assert!(!home.0.join(".config/kkt/access-key").exists());
}

#[cfg(unix)]
#[test]
fn claude_write_uses_its_cli_and_is_idempotent_with_backup_and_other_fields_kept() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let original = r#"{"theme":"keep","mcpServers":{"other":{"command":"other"},"kkt":{"url":"old","type":"http","timeout":123}}}"#;
    home.put(".claude.json", original);
    let script = home.put(
        "bin/claude",
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
home = pathlib.Path(os.environ['HOME'])
args = sys.argv[1:]
assert args[:1] == ['mcp'] and '--scope' in args and 'user' in args
with (home / 'calls').open('a') as log: log.write(args[1] + '\n')
file = home / '.claude.json'
doc = json.loads(file.read_text()) if file.exists() else {}
servers = doc.setdefault('mcpServers', {})
if args[1] == 'remove': servers.pop('kkt', None)
elif args[1] == 'add-json': servers['kkt'] = json.loads(args[-1])
else: sys.exit(9)
file.write_text(json.dumps(doc))
"#,
    );
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let key = home.put("key", KEY);
    let path = format!(
        "{}:{}",
        home.0.join("bin").display(),
        std::env::var("PATH").expect("PATH")
    );
    let run = || {
        home.command()
            .args([
                "connect",
                "claude-code",
                "--key-file",
                key.to_str().expect("key"),
                "--write",
            ])
            .env("PATH", &path)
            .output()
            .expect("kkt")
    };
    let out = run();
    assert!(out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains(KEY));
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.0.join(".claude.json")).expect("config"),
    )
    .expect("JSON");
    assert_eq!(config["theme"], "keep");
    assert_eq!(config["mcpServers"]["other"]["command"], "other");
    assert_eq!(config["mcpServers"]["kkt"]["timeout"], 123);
    assert!(
        config["mcpServers"]["kkt"]["headersHelper"]
            .as_str()
            .expect("helper")
            .contains("key header --json")
    );
    assert_eq!(
        std::fs::read_to_string(home.0.join(".claude.json.bak")).expect("backup"),
        original
    );
    assert!(run().status.success());
    assert_eq!(
        std::fs::read_to_string(home.0.join("calls")).expect("calls"),
        "remove\nadd-json\n"
    );
}

#[test]
fn missing_claude_binary_is_clear_and_does_not_change_the_original_config() {
    let home = Home::new();
    let original = r#"{"mcpServers":{"kkt":{"url":"old","type":"http"}}}"#;
    home.put(".claude.json", original);
    std::fs::create_dir(home.0.join("empty-bin")).expect("bin");
    let out = home
        .command()
        .args(["connect", "claude-code", "--write"])
        .env("PATH", home.0.join("empty-bin"))
        .output()
        .expect("kkt");
    assert!(!out.status.success());
    assert!(
        text(&out).contains("claude was not found on PATH"),
        "{}",
        text(&out)
    );
    assert_eq!(
        std::fs::read_to_string(home.0.join(".claude.json")).expect("config"),
        original
    );
}

#[cfg(unix)]
#[test]
fn failed_claude_replacement_restores_original_and_does_not_echo_client_output() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let original = r#"{"theme":"keep","mcpServers":{"kkt":{"url":"old","type":"http"}}}"#;
    home.put(".claude.json", original);
    let script = home.put(
        "bin/claude",
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
file = pathlib.Path(os.environ['HOME']) / '.claude.json'
if sys.argv[2] == 'remove':
    doc = json.loads(file.read_text())
    doc['mcpServers'].pop('kkt')
    file.write_text(json.dumps(doc))
    sys.exit(0)
print('0123456789abcdef0123456789abcdef')
sys.exit(9)
"#,
    );
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let path = format!(
        "{}:{}",
        home.0.join("bin").display(),
        std::env::var("PATH").expect("PATH")
    );
    let out = home
        .command()
        .args(["connect", "claude-code", "--write"])
        .env("PATH", path)
        .output()
        .expect("kkt");
    assert!(!out.status.success());
    assert!(!text(&out).contains(KEY));
    assert_eq!(
        std::fs::read_to_string(home.0.join(".claude.json")).expect("restored"),
        original
    );
    assert_eq!(
        std::fs::read_to_string(home.0.join(".claude.json.bak")).expect("backup"),
        original
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn doctor_cli_reports_typed_service_state_without_real_manager_commands() {
    use std::os::unix::fs::PermissionsExt;
    let server = common::start().await;
    let home = Home::new();
    home.put(".config/kkt/config.toml", "no_access_key = true\n");
    home.put(".config/systemd/user/kashkeshet.service", "test unit");
    let script = home.put("bin/systemctl", "#!/bin/sh\nexit 0\n");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let path = format!(
        "{}:{}",
        home.0.join("bin").display(),
        std::env::var("PATH").expect("PATH")
    );
    let url = format!("{}/mcp", server.base);
    let run = || {
        let mut command = tokio::process::Command::from(home.command());
        command.args(["doctor", "--url", &url]).env("PATH", &path);
        command
    };
    let out = run().output().await.expect("doctor");
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("ok: this machine's kkt service: installed and running"),
        "{}",
        text(&out)
    );
    std::fs::write(&script, "#!/bin/sh\nexit 3\n").expect("inactive");
    let out = run().output().await.expect("doctor");
    assert!(!out.status.success());
    assert!(text(&out).contains("problem: this machine's kkt service: installed but not running"));
    assert!(text(&out).contains("kkt service start"));
    std::fs::remove_file(home.0.join(".config/systemd/user/kashkeshet.service"))
        .expect("remove unit");
    let out = run().output().await.expect("doctor");
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("not installed; optional"));
}
