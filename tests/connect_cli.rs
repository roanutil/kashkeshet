//! CLI integration uses an isolated HOME and a fake Claude executable.

#[path = "common/home.rs"]
mod home;
use home::OwnHome;

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
        command.own_home(&self.0).current_dir(&self.0);
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

#[cfg(unix)]
fn profile_claude(home: &Home) -> String {
    use std::os::unix::fs::PermissionsExt;
    let script = home.put(
        "bin/claude",
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
home = pathlib.Path(os.environ.get('HOME', os.getcwd()))
profile = os.environ.get('CLAUDE_CONFIG_DIR')
root = pathlib.Path(profile) if profile else home / '.claude'
file = (pathlib.Path(profile) if profile else home) / '.claude.json'
assert sys.argv[1:3] in [['mcp', 'add-json'], ['mcp', 'remove']]
assert sys.argv[3:6] == ['--scope', 'user', 'kkt']
failure = os.environ.get('KKT_TEST_FAIL_ADD')
if failure == 'before': sys.exit(9)
doc = json.loads(file.read_text()) if file.exists() else {}
servers = doc.setdefault('mcpServers', {})
if sys.argv[2] == 'remove': servers.pop('kkt', None)
else: servers['kkt'] = json.loads(sys.argv[-1])
backups = root / 'backups'
backups.mkdir(parents=True, exist_ok=True)
if file.exists(): (backups / '.claude.json.backup.fixture').write_bytes(file.read_bytes())
file.parent.mkdir(parents=True, exist_ok=True)
file.write_text(json.dumps(doc))
if failure == 'after':
    print('0123456789abcdef0123456789abcdef')
    sys.exit(9)
"#,
    );
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("executable");
    format!(
        "{}:{}",
        home.0.join("bin").display(),
        std::env::var("PATH").expect("PATH")
    )
}

#[test]
fn client_config_overrides_select_file_client_previews_writes_and_backups() {
    // Codex and Muse Code read only a file of their fixed name; the others
    // take any name.
    for (client, default, name, group, url_field) in [
        (
            "codex",
            ".codex/config.toml",
            "config.toml",
            "mcp_servers",
            "url",
        ),
        (
            "agy",
            ".gemini/config/mcp_config.json",
            "client.conf",
            "mcpServers",
            "serverUrl",
        ),
        (
            "opencode",
            ".config/opencode/opencode.json",
            "client.conf",
            "mcp",
            "url",
        ),
        (
            "muse-code",
            ".config/muse/settings.json",
            "settings.json",
            "mcpServers",
            "url",
        ),
    ] {
        for (relative, existing, has_home) in [
            (true, true, true),
            (false, true, true),
            (true, false, true),
            (false, false, false),
        ] {
            let home = Home::new();
            let untouched = "default config must not be read or modified";
            let default_file = home.put(default, untouched);
            let key = home.put("key", KEY);
            let file = home.0.join("custom profile").join(name);
            let selected = if relative {
                PathBuf::from("custom profile").join(name)
            } else {
                file.clone()
            };
            let original = if client == "codex" {
                "# keep comment\nlabel = 'keep'\n[mcp_servers.other]\nurl = 'https://other/mcp'\n"
                    .to_string()
            } else {
                serde_json::json!({"label": "keep", group: {"other": {"url": "https://other/mcp"}}})
                    .to_string()
            };
            if existing {
                home.put(&format!("custom profile/{name}"), &original);
            }
            let run = |write| {
                let mut command = home.command();
                command
                    .args(["connect", client, "--client-config"])
                    .arg(&selected)
                    .arg("--key-file")
                    .arg(&key)
                    .args(["--url", "https://selected.example/mcp"]);
                if !has_home {
                    command.env_remove("HOME");
                }
                if write {
                    command.arg("--write");
                }
                command.output().expect("kkt")
            };
            let backup = file.with_file_name(format!("{name}.bak"));
            let preview = run(false);
            assert!(preview.status.success(), "{client}: {}", text(&preview));
            assert!(text(&preview).contains(&format!("Config: {}", selected.display())));
            assert!(text(&preview).contains("https://selected.example/mcp"));
            assert!(!text(&preview).contains(KEY));
            assert_eq!(
                std::fs::read_to_string(&file).ok(),
                existing.then(|| original.clone())
            );
            assert!(!backup.exists());
            let written = run(true);
            assert!(written.status.success(), "{client}: {}", text(&written));
            assert!(!text(&written).contains(KEY));
            let contents = std::fs::read_to_string(&file).expect("custom config");
            let doc: serde_json::Value = if client == "codex" {
                serde_json::to_value(toml::from_str::<toml::Value>(&contents).expect("TOML"))
                    .expect("JSON")
            } else {
                serde_json::from_str(&contents).expect("JSON")
            };
            assert_eq!(doc[group]["kkt"][url_field], "https://selected.example/mcp");
            if existing {
                assert_eq!(doc["label"], "keep");
                assert_eq!(doc[group]["other"]["url"], "https://other/mcp");
                assert_eq!(std::fs::read_to_string(&backup).expect("backup"), original);
                if client == "codex" {
                    assert!(contents.contains("# keep comment"));
                }
            } else {
                assert!(!backup.exists());
            }
            let again = run(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for private_file in [&file, &backup].into_iter().filter(|path| path.exists()) {
                    assert_eq!(
                        std::fs::metadata(private_file)
                            .expect("private file")
                            .permissions()
                            .mode()
                            & 0o777,
                        0o600
                    );
                }
            }
            assert!(again.status.success(), "{client}: {}", text(&again));
            assert!(text(&again).contains("Already configured"));
            assert_eq!(std::fs::read_to_string(&file).expect("unchanged"), contents);
            assert!(!file.with_file_name(format!("{name}.bak.1")).exists());
            assert_eq!(
                std::fs::read_to_string(&default_file).expect("default"),
                untouched
            );
            assert!(
                !default_file
                    .with_file_name(format!(
                        "{}.bak",
                        default_file
                            .file_name()
                            .expect("filename")
                            .to_string_lossy()
                    ))
                    .exists()
            );
        }
    }
}

#[test]
fn connect_writes_where_codex_home_and_xdg_config_home_point() {
    let home = Home::new();
    let codex_home = home.0.join("codex-profile");
    let xdg = home.0.join("xdg");
    for (client, file) in [
        ("codex", codex_home.join("config.toml")),
        ("opencode", xdg.join("opencode/opencode.json")),
        ("muse-code", xdg.join("muse/settings.json")),
    ] {
        let out = home
            .command()
            .args(["connect", client, "--write"])
            .env("CODEX_HOME", &codex_home)
            .env("XDG_CONFIG_HOME", &xdg)
            .output()
            .expect("run kkt");
        assert!(out.status.success(), "{client}: {}", text(&out));
        assert!(
            text(&out).contains(&file.display().to_string()),
            "{client}: {}",
            text(&out)
        );
        assert!(
            std::fs::read_to_string(&file)
                .expect("written config")
                .contains("kkt"),
            "{client}"
        );
    }
    assert!(!home.0.join(".codex").exists());
    assert!(!home.0.join(".config/opencode").exists());
    assert!(!home.0.join(".config/muse").exists());
}

#[test]
fn file_errors_name_the_variable_that_chose_the_path() {
    let home = Home::new();
    let codex_home = home.0.join("codex-profile");
    let xdg = home.0.join("xdg");
    // A directory where the config file should be cannot be read.
    std::fs::create_dir_all(codex_home.join("config.toml")).expect("codex dir");
    std::fs::create_dir_all(xdg.join("opencode/opencode.json")).expect("opencode dir");
    std::fs::create_dir_all(xdg.join("muse/settings.json")).expect("muse dir");
    for (client, variable) in [
        ("codex", "CODEX_HOME"),
        ("opencode", "XDG_CONFIG_HOME"),
        ("muse-code", "XDG_CONFIG_HOME"),
    ] {
        let out = home
            .command()
            .args(["connect", client])
            .env("CODEX_HOME", &codex_home)
            .env("XDG_CONFIG_HOME", &xdg)
            .output()
            .expect("run kkt");
        assert!(!out.status.success(), "{client}: {}", text(&out));
        assert!(
            text(&out).contains(&format!("(from {variable})")),
            "{client}: {}",
            text(&out)
        );
    }
}

#[test]
fn a_wrong_config_name_is_refused_before_a_key_is_created() {
    let home = Home::new();
    for client in ["codex", "claude-code", "muse-code"] {
        let out = home.run(&["connect", client, "--client-config", "wrong.toml"]);
        assert!(!out.status.success(), "{client}: {}", text(&out));
        assert!(text(&out).contains("must be named"), "{}", text(&out));
        assert!(!home.0.join(".config/kkt/access-key").exists(), "{client}");
    }
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
        .own_home(&home.0)
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
        .own_home(&home.0)
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
root = pathlib.Path(os.environ.get('CLAUDE_CONFIG_DIR') or home / '.claude')
(root / 'backups').mkdir(parents=True, exist_ok=True)
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
    assert!(home.0.join(".claude/backups").is_dir());
    assert!(!home.0.join("backups").exists());
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

/// Make `dir` read-only; false if writing there still works (as root).
#[cfg(unix)]
fn read_only(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o500)).expect("read-only");
    std::fs::write(dir.join("probe"), "").is_err()
}

#[cfg(unix)]
fn writable(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).expect("writable");
}

#[cfg(unix)]
#[test]
fn backup_failures_name_the_variable_that_chose_the_path() {
    let home = Home::new();
    let profile = home.0.join("profile");
    for (client, variable, file, original) in [
        ("codex", "CODEX_HOME", "config.toml", ""),
        ("claude-code", "CLAUDE_CONFIG_DIR", ".claude.json", "{}"),
    ] {
        home.put(&format!("profile/{file}"), original);
        if !read_only(&profile) {
            writable(&profile);
            return;
        }
        let out = home
            .command()
            .args(["connect", client, "--write"])
            .env(variable, &profile)
            .output()
            .expect("run kkt");
        writable(&profile);
        assert!(!out.status.success(), "{client}: {}", text(&out));
        assert!(
            text(&out).contains("could not back up"),
            "{client}: {}",
            text(&out)
        );
        assert!(
            text(&out).contains(&format!("(from {variable})")),
            "{client}: {}",
            text(&out)
        );
        std::fs::remove_dir_all(&profile).expect("reset profile");
    }
}

#[cfg(unix)]
#[test]
fn a_failed_claude_restore_reports_both_errors_and_the_variable() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let profile = home.0.join("profile");
    home.put("profile/.claude.json", "{}");
    if !read_only(&profile) {
        writable(&profile);
        return;
    }
    writable(&profile);
    // Claude fails after its profile becomes read-only, so kkt cannot restore.
    let script = home.put(
        "bin/claude",
        "#!/bin/sh\nchmod 500 \"$CLAUDE_CONFIG_DIR\"\nexit 9\n",
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
        .env("CLAUDE_CONFIG_DIR", &profile)
        .env("PATH", path)
        .output()
        .expect("kkt");
    writable(&profile);
    assert!(!out.status.success());
    let out = text(&out);
    assert!(out.contains("claude mcp failed"), "{out}");
    assert!(out.contains("could not restore"), "{out}");
    assert!(out.contains("(from CLAUDE_CONFIG_DIR)"), "{out}");
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

#[cfg(unix)]
#[test]
fn claude_profile_selection_controls_preview_write_backup_and_rollback() {
    for (explicit, fail) in [(false, false), (true, false), (false, true), (true, true)] {
        let home = Home::new();
        let untouched = r#"{"mcpServers":{"kkt":{"url":"home"}}}"#;
        home.put(".claude.json", untouched);
        let original = r#"{"theme":"keep","mcpServers":{"kkt":{"url":"old","type":"http"}}}"#;
        let profile = home.put("profile/.claude.json", original);
        let other = home.put("other/.claude.json", original);
        let selected = if explicit { &other } else { &profile };
        let path = profile_claude(&home);
        let run = |write| {
            let mut command = home.command();
            command
                .args(["connect", "claude-code"])
                .env("CLAUDE_CONFIG_DIR", "profile")
                .env("PATH", &path)
                .env("KKT_TEST_FAIL_ADD", if fail { "after" } else { "" });
            if explicit {
                command.arg("--client-config").arg(&other);
            }
            if write {
                command.arg("--write");
            }
            command.output().expect("kkt")
        };
        let preview = run(false);
        assert!(preview.status.success(), "{}", text(&preview));
        let preview_path = if explicit {
            other.clone()
        } else {
            PathBuf::from("profile/.claude.json")
        };
        assert!(
            text(&preview).contains(&format!("Config: {}", preview_path.display())),
            "{}",
            text(&preview)
        );
        assert_eq!(
            std::fs::read_to_string(selected).expect("preview unchanged"),
            original
        );
        let out = run(true);
        assert_eq!(out.status.success(), !fail, "{}", text(&out));
        assert_eq!(
            std::fs::read_to_string(home.0.join(".claude.json")).expect("home"),
            untouched
        );
        assert!(!home.0.join(".claude.json.bak").exists());
        assert_eq!(
            std::fs::read_to_string(selected.with_file_name(".claude.json.bak")).expect("backup"),
            original
        );
        if fail {
            assert_eq!(
                std::fs::read_to_string(selected).expect("restored"),
                original
            );
        } else {
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(selected).expect("updated"))
                    .expect("JSON");
            assert_eq!(doc["theme"], "keep");
            assert_ne!(doc["mcpServers"]["kkt"]["url"], "old");
            let again = run(true);
            assert!(again.status.success(), "{}", text(&again));
            assert!(text(&again).contains("Already configured"));
            assert!(!selected.with_file_name(".claude.json.bak.1").exists());
        }
        let unselected = if explicit { &profile } else { &other };
        assert_eq!(
            std::fs::read_to_string(unselected).expect("unselected"),
            original
        );
    }
}

#[cfg(unix)]
#[test]
fn claude_preview_command_writes_the_selected_profile() {
    for (name, inherited) in [
        ("profile with ' quote/.claude.json", None),
        ("profile with ' quote/.claude.json", Some("other")),
        (".claude.json", Some("other")),
        (".claude.json", None),
    ] {
        let home = Home::new();
        let profile = home.0.join(name);
        let path = profile_claude(&home);
        let mut command = home.command();
        command
            .args(["connect", "claude-code", "--client-config"])
            .arg(&profile);
        if let Some(dir) = inherited {
            command.env("CLAUDE_CONFIG_DIR", dir);
        }
        let preview = command.output().expect("preview");
        assert!(preview.status.success(), "{}", text(&preview));
        let stdout = String::from_utf8(preview.stdout).expect("UTF-8");
        let mut lines = stdout
            .lines()
            .skip_while(|line| !line.starts_with("Config: "));
        lines.next().expect("config destination");
        let shell_command = lines.next().expect("preview command");
        let mut shell = Command::new("sh");
        shell
            .args(["-c", shell_command])
            .own_home(&home.0)
            .env("PATH", &path)
            .current_dir(&home.0);
        if let Some(dir) = inherited {
            shell.env("CLAUDE_CONFIG_DIR", dir);
        }
        let output = shell.output().expect("execute preview");
        assert!(output.status.success(), "{}", text(&output));
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&profile).expect("selected profile written"),
        )
        .expect("JSON");
        assert!(doc["mcpServers"]["kkt"]["url"].is_string());
        if name != ".claude.json" {
            assert!(!home.0.join(".claude.json").exists());
        }
        assert!(!home.0.join("other/.claude.json").exists());
    }
}

#[cfg(unix)]
#[test]
fn claude_preview_and_write_preserve_the_selected_settings_root() {
    for (inherited, explicit, selected, root, has_home) in [
        (Some("."), None, ".claude.json", ".", true),
        (
            Some("profile"),
            Some(".claude.json"),
            ".claude.json",
            ".claude",
            true,
        ),
        (
            None,
            Some("profile with ' quote/.claude.json"),
            "profile with ' quote/.claude.json",
            "profile with ' quote",
            true,
        ),
        (
            Some("profile"),
            None,
            "profile/.claude.json",
            "profile",
            true,
        ),
        (Some(""), None, ".claude.json", ".claude", true),
        (
            Some("profile"),
            None,
            "profile/.claude.json",
            "profile",
            false,
        ),
        (
            None,
            Some("profile/.claude.json"),
            "profile/.claude.json",
            "profile",
            false,
        ),
    ] {
        let home = Home::new();
        let path = profile_claude(&home);
        let original = r#"{"theme":"keep","mcpServers":{"other":{"url":"https://other/mcp"},"kkt":{"url":"old","type":"http"}}}"#;
        let file = home.put(selected, original);
        let other = home.put(
            if selected == ".claude.json" {
                "profile/.claude.json"
            } else {
                ".claude.json"
            },
            "untouched",
        );
        let key = home.put("key", KEY);
        let run = |write| {
            let mut command = home.command();
            command
                .args(["connect", "claude-code", "--key-file"])
                .arg(&key)
                .env("PATH", &path);
            if let Some(dir) = inherited {
                command.env("CLAUDE_CONFIG_DIR", dir);
            }
            if let Some(config) = explicit {
                command.args(["--client-config", config]);
            }
            if !has_home {
                command.env_remove("HOME");
            }
            command.args([
                "--url",
                if write {
                    "https://direct.example/mcp"
                } else {
                    "https://preview.example/mcp"
                },
            ]);
            if write {
                command.arg("--write");
            }
            command.output().expect("kkt")
        };
        let preview = run(false);
        assert!(preview.status.success(), "{selected}: {}", text(&preview));
        assert_eq!(
            std::fs::read_to_string(&file).expect("preview unchanged"),
            original
        );
        let stdout = String::from_utf8(preview.stdout).expect("UTF-8");
        let mut lines = stdout
            .lines()
            .skip_while(|line| !line.starts_with("Config: "));
        lines.next().expect("destination");
        let command = lines.next().expect("preview command");
        let mut shell = Command::new("sh");
        shell
            .args(["-c", command])
            .current_dir(&home.0)
            .env("PATH", &path)
            .own_home(&home.0);
        if let Some(dir) = inherited {
            shell.env("CLAUDE_CONFIG_DIR", dir);
        }
        if !has_home {
            shell.env_remove("HOME");
        }
        let output = shell.output().expect("execute preview");
        assert!(output.status.success(), "{selected}: {}", text(&output));
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).expect("selected config"))
                .expect("JSON");
        assert_eq!(
            doc["mcpServers"]["kkt"]["url"],
            "https://preview.example/mcp"
        );
        assert_eq!(
            std::fs::read_to_string(
                home.0
                    .join(root)
                    .join("backups/.claude.json.backup.fixture")
            )
            .unwrap_or_else(|e| panic!("Claude backup: selected={selected}, root={root}, inherited={inherited:?}: {e}; preview={stdout}")),
            original
        );
        let output = run(true);
        assert!(output.status.success(), "{selected}: {}", text(&output));
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).expect("selected config"))
                .expect("JSON");
        assert_eq!(
            doc["mcpServers"]["kkt"]["url"],
            "https://direct.example/mcp"
        );
        assert_eq!(doc["theme"], "keep");
        assert_eq!(doc["mcpServers"]["other"]["url"], "https://other/mcp");
        assert_eq!(
            std::fs::read_to_string(&other).expect("unselected config"),
            "untouched"
        );
        if root != "." {
            assert!(!home.0.join("backups").exists());
        }
        if root != ".claude" {
            assert!(!home.0.join(".claude/backups").exists());
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn claude_non_utf8_profile_can_be_written_but_not_previewed() {
    use std::os::unix::ffi::OsStringExt;
    let home = Home::new();
    let path = profile_claude(&home);
    let directory = home
        .0
        .join(std::ffi::OsString::from_vec(b"profile-\xff".to_vec()));
    std::fs::create_dir(&directory).expect("profile directory");
    let file = directory.join(".claude.json");
    let original = r#"{"theme":"keep"}"#;
    std::fs::write(&file, original).expect("original config");
    let run = |write| {
        let mut command = home.command();
        command
            .args(["connect", "claude-code", "--client-config"])
            .arg(&file)
            .env("PATH", &path);
        if write {
            command.arg("--write");
        }
        command.output().expect("kkt")
    };
    let preview = run(false);
    assert!(!preview.status.success());
    assert!(
        text(&preview).contains("is not UTF-8 (from --client-config)"),
        "{}",
        text(&preview)
    );
    assert!(!text(&preview).contains(KEY));
    assert_eq!(std::fs::read_to_string(&file).expect("unchanged"), original);
    assert!(!directory.join(".claude.json.bak").exists());
    let written = run(true);
    assert!(written.status.success(), "{}", text(&written));
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("written config"))
            .expect("JSON");
    assert_eq!(doc["theme"], "keep");
    assert_eq!(doc["mcpServers"]["kkt"]["url"], "http://127.0.0.1:7420/mcp");
    assert_eq!(
        std::fs::read_to_string(directory.join(".claude.json.bak")).expect("backup"),
        original
    );
    assert!(!home.0.join(".claude.json").exists());
}

#[cfg(unix)]
#[test]
fn claude_new_profile_is_written_only_when_add_succeeds() {
    for failure in [None, Some("before"), Some("after")] {
        let home = Home::new();
        let path = profile_claude(&home);
        let original = r#"{"mcpServers":{"kkt":{"url":"untouched"}}}"#;
        let default = home.put(".claude.json", original);
        let file = home.0.join("new profile/.claude.json");
        let mut command = home.command();
        command
            .args([
                "connect",
                "claude-code",
                "--client-config",
                "new profile/.claude.json",
                "--write",
            ])
            .env("PATH", &path);
        if let Some(failure) = failure {
            command.env("KKT_TEST_FAIL_ADD", failure);
        }
        let output = command.output().expect("kkt");
        assert_eq!(
            output.status.success(),
            failure.is_none(),
            "{}",
            text(&output)
        );
        if failure.is_some() {
            assert!(
                text(&output).contains("claude mcp failed with"),
                "{}",
                text(&output)
            );
            assert!(!file.exists());
        } else {
            let doc: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&file).expect("new profile config"))
                    .expect("JSON");
            assert_eq!(doc["mcpServers"]["kkt"]["url"], "http://127.0.0.1:7420/mcp");
        }
        assert!(!text(&output).contains(KEY));
        assert!(!file.with_file_name(".claude.json.bak").exists());
        assert_eq!(
            std::fs::read_to_string(default).expect("default unchanged"),
            original
        );
        assert!(!home.0.join(".claude.json.bak").exists());
    }
}

#[cfg(unix)]
#[test]
fn claude_default_config_preserves_settings_roots_through_a_symlinked_home() {
    use std::os::unix::fs::symlink;
    for inherited in [false, true] {
        let home = Home::new();
        let path = profile_claude(&home);
        let alias = home.0.join("home-alias");
        symlink(&home.0, &alias).expect("home symlink");
        let original = r#"{"theme":"keep"}"#;
        let file = home.put(".claude.json", original);
        let mut command = home.command();
        command
            .args([
                "connect",
                "claude-code",
                "--client-config",
                ".claude.json",
                "--write",
            ])
            .own_home(&alias)
            .env("PATH", &path);
        if inherited {
            command.env("CLAUDE_CONFIG_DIR", &alias);
        }
        let output = command.output().expect("kkt");
        assert!(output.status.success(), "{}", text(&output));
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).expect("written config"))
                .expect("JSON");
        assert_eq!(doc["theme"], "keep");
        assert_eq!(doc["mcpServers"]["kkt"]["url"], "http://127.0.0.1:7420/mcp");
        let expected_root = if inherited {
            &home.0
        } else {
            &home.0.join(".claude")
        };
        assert_eq!(
            std::fs::read_to_string(expected_root.join("backups/.claude.json.backup.fixture"))
                .expect("Claude backup"),
            original
        );
        let unexpected_backup = if inherited {
            home.0.join(".claude/backups")
        } else {
            home.0.join("backups")
        };
        assert!(!unexpected_backup.exists());
    }
}

#[cfg(unix)]
#[test]
fn claude_explicit_config_wins_over_an_inherited_profile_it_cannot_resolve() {
    let home = Home::new();
    let path = profile_claude(&home);
    let not_a_directory = home.put("plain-file", "");
    let file = home.put("work/.claude.json", r#"{"theme":"keep"}"#);
    for write in [false, true] {
        let mut command = home.command();
        command
            .args(["connect", "claude-code", "--client-config"])
            .arg(&file)
            .env("CLAUDE_CONFIG_DIR", not_a_directory.join("profile"))
            .env("PATH", &path);
        if write {
            command.arg("--write");
        }
        let output = command.output().expect("kkt");
        assert!(output.status.success(), "{}", text(&output));
    }
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("written config"))
            .expect("JSON");
    assert_eq!(doc["theme"], "keep");
    assert_eq!(doc["mcpServers"]["kkt"]["url"], "http://127.0.0.1:7420/mcp");
    assert!(!home.0.join(".claude.json").exists());
}
