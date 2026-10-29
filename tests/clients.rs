use kashkeshet::clients::{self, Client, Setup};
use kashkeshet::settings::Env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const KEY: &str = "0123456789abcdef0123456789abcdef";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Home(PathBuf);
impl Home {
    fn new() -> Home {
        let path = std::env::temp_dir().join(format!(
            "kkt-clients-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("home");
        Home(path)
    }

    fn env(&self) -> Env {
        Env {
            home: Some(self.0.clone()),
            claude_config_dir: None,
            codex_home: None,
            xdg_config_home: None,
            xdg_data_home: None,
        }
    }

    fn put(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        std::fs::write(&path, text).expect("file");
        path
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("cleanup");
    }
}

#[test]
fn previews_use_vendor_shapes_and_never_show_the_key() {
    let home = Home::new();
    let key = home.put("key", &format!("{KEY}\n"));
    let env = home.env();
    for (client, required) in [
        (Client::ClaudeCode, "headersHelper"),
        (Client::Codex, "http_headers_helper"),
        (Client::Agy, "serverUrl"),
        (Client::OpenCode, "{file:"),
        (Client::MuseCode, "streamable_http"),
    ] {
        let client_config = client.file(&env).expect("client config");
        let setup = Setup {
            env: &env,
            url: "http://127.0.0.1:7420/mcp",
            executable: Path::new("/path with ' quote/kkt"),
            key_file: Some(&key),
            client_config: &client_config,
            source: "HOME",
        };
        let out = clients::configure(client, &setup, false).expect("preview");
        assert!(out.contains(required), "{out}");
        assert!(!out.contains(KEY), "preview exposed key");
        assert!(!client_config.exists());
    }
    let out = clients::header(KEY, true).expect("header");
    let header: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(header["Authorization"], format!("Bearer {KEY}"));
    assert_eq!(
        clients::header(KEY, false).expect("header"),
        format!("Authorization: Bearer {KEY}")
    );
}

#[test]
fn clients_with_a_fixed_filename_reject_custom_filenames_before_modifying_files() {
    for (client, original, required) in [
        (Client::ClaudeCode, "{}", ".claude.json"),
        (Client::Codex, "", "config.toml"),
        (Client::MuseCode, "{}", "settings.json"),
    ] {
        let home = Home::new();
        let file = home.put("profile/custom.conf", original);
        let env = home.env();
        let setup = Setup {
            env: &env,
            url: "http://localhost:7420/mcp",
            executable: Path::new("/opt/kkt"),
            key_file: None,
            client_config: &file,
            source: "--client-config",
        };
        for write in [false, true] {
            let error =
                clients::configure(client, &setup, write).expect_err("unsupported filename");
            assert!(
                error.contains(&format!("must be named {required}")),
                "{error}"
            );
            assert!(error.contains("(from --client-config)"), "{error}");
            assert_eq!(std::fs::read_to_string(&file).expect("config"), original);
            assert!(!file.with_file_name("custom.conf.bak").exists());
            assert!(!file.with_file_name(required).exists());
        }
    }
}

#[test]
fn codex_merge_preserves_comments_other_servers_and_options_and_is_idempotent() {
    let home = Home::new();
    let original = "# keep my comment\nmodel = 'custom'\n[mcp_servers.other]\ncommand = 'other'\n[mcp_servers.kkt]\nurl = 'http://old/mcp'\nbearer_token_env_var = 'OLD_KEY'\ntool_timeout_sec = 91\n";
    let file = home.put(".codex/config.toml", original);
    let key = home.put("key", KEY);
    let env = home.env();
    let setup = Setup {
        env: &env,
        url: "https://tail.example/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: Some(&key),
        client_config: &file,
        source: "--client-config",
    };
    clients::configure(Client::Codex, &setup, true).expect("write");
    let first = std::fs::read_to_string(&file).expect("config");
    assert!(first.contains("# keep my comment"));
    let parsed: toml::Value = toml::from_str(&first).expect("TOML");
    assert_eq!(parsed["model"].as_str(), Some("custom"));
    assert_eq!(
        parsed["mcp_servers"]["other"]["command"].as_str(),
        Some("other")
    );
    assert_eq!(
        parsed["mcp_servers"]["kkt"]["tool_timeout_sec"].as_integer(),
        Some(91)
    );
    assert!(
        parsed["mcp_servers"]["kkt"]
            .get("bearer_token_env_var")
            .is_none()
    );
    assert!(
        parsed["mcp_servers"]["kkt"]["http_headers_helper"]
            .as_str()
            .expect("helper")
            .contains("key header --json")
    );
    assert_eq!(
        std::fs::read_to_string(file.with_file_name("config.toml.bak")).expect("backup"),
        original
    );
    let out = clients::configure(Client::Codex, &setup, true).expect("again");
    assert!(out.contains("Already configured"));
    assert_eq!(std::fs::read_to_string(&file).expect("config"), first);
    assert!(!file.with_file_name("config.toml.bak.1").exists());
}

#[test]
fn json_clients_merge_without_losing_other_settings_and_back_up_once() {
    for (client, name, group) in [
        (Client::Agy, ".gemini/config/mcp_config.json", "mcpServers"),
        (Client::OpenCode, ".config/opencode/opencode.json", "mcp"),
    ] {
        let home = Home::new();
        let original = serde_json::json!({"theme":"custom", group:{"other":{"url":"https://other"},"kkt":{"enabled":true,"headers":{"X-Custom":"keep","authorization":"stale"}}}}).to_string();
        let file = home.put(name, &original);
        let key = home.put("key", &format!("{KEY}\n"));
        let env = home.env();
        let setup = Setup {
            env: &env,
            url: "http://localhost:7420/mcp",
            executable: Path::new("/opt/kkt"),
            key_file: Some(&key),
            client_config: &file,
            source: "--client-config",
        };
        let out = clients::configure(client, &setup, true).expect("write");
        assert!(!out.contains(KEY));
        let first = std::fs::read_to_string(&file).expect("config");
        let parsed: serde_json::Value = serde_json::from_str(&first).expect("JSON");
        assert_eq!(parsed["theme"], "custom");
        assert_eq!(parsed[group]["other"]["url"], "https://other");
        assert_eq!(parsed[group]["kkt"]["enabled"], true);
        assert_eq!(parsed[group]["kkt"]["headers"]["X-Custom"], "keep");
        assert!(
            parsed[group]["kkt"]["headers"]
                .get("authorization")
                .is_none()
        );
        if client == Client::Agy {
            assert_eq!(
                parsed[group]["kkt"]["headers"]["Authorization"],
                format!("Bearer {KEY}")
            );
        } else {
            assert_eq!(parsed[group]["kkt"]["oauth"], false);
            assert!(
                parsed[group]["kkt"]["headers"]["Authorization"]
                    .as_str()
                    .expect("file")
                    .contains("{file:")
            );
            assert!(!first.contains(KEY));
        }
        clients::configure(client, &setup, true).expect("again");
        assert_eq!(std::fs::read_to_string(&file).expect("config"), first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).expect("mode").permissions().mode() & 0o777,
                0o600
            );
            let backup = file.with_file_name(format!(
                "{}.bak",
                file.file_name().expect("name").to_string_lossy()
            ));
            assert_eq!(std::fs::read_to_string(&backup).expect("backup"), original);
            assert_eq!(
                std::fs::metadata(backup)
                    .expect("mode")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn muse_code_merge_keeps_other_settings_adds_schema_version_and_is_idempotent() {
    let home = Home::new();
    // No schema_version yet: Muse Code refuses the file without it.
    let original = r#"{"model":"keep","mcpServers":{"other":{"url":"https://other"},"kkt":{"transport":"stdio","command":"old-kkt","headers":{"X-Custom":"keep","authorization":"stale"}}}}"#;
    let file = home.put(".config/muse/settings.json", original);
    let key = home.put("key", &format!("{KEY}\n"));
    let env = home.env();
    assert_eq!(Client::MuseCode.file(&env).expect("Muse path"), file);
    let setup = Setup {
        env: &env,
        url: "http://localhost:7420/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: Some(&key),
        client_config: &file,
        source: "HOME",
    };
    let preview = clients::configure(Client::MuseCode, &setup, false).expect("preview");
    assert!(preview.contains("Muse Code stores the key"), "{preview}");
    assert!(!preview.contains(KEY), "preview exposed key");
    let out = clients::configure(Client::MuseCode, &setup, true).expect("write");
    assert!(out.contains("Muse Code stores the key"), "{out}");
    assert!(!out.contains(KEY), "output exposed key");
    let first = std::fs::read_to_string(&file).expect("config");
    let parsed: serde_json::Value = serde_json::from_str(&first).expect("JSON");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["model"], "keep");
    assert_eq!(parsed["mcpServers"]["other"]["url"], "https://other");
    let entry = &parsed["mcpServers"]["kkt"];
    assert_eq!(entry["transport"], "streamable_http");
    assert_eq!(entry["url"], "http://localhost:7420/mcp");
    assert_eq!(entry["mode"], "optional");
    assert_eq!(entry["headers"]["X-Custom"], "keep");
    assert_eq!(entry["headers"]["Authorization"], format!("Bearer {KEY}"));
    assert!(entry.get("command").is_none());
    assert!(entry["headers"].get("authorization").is_none());
    assert_eq!(
        std::fs::read_to_string(home.0.join(".config/muse/settings.json.bak")).expect("backup"),
        original
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).expect("mode").permissions().mode() & 0o777,
            0o600
        );
    }
    let again = clients::configure(Client::MuseCode, &setup, true).expect("again");
    assert!(again.contains("Already configured"), "{again}");
    assert_eq!(std::fs::read_to_string(&file).expect("config"), first);
    assert!(!home.0.join(".config/muse/settings.json.bak.1").exists());
}

#[test]
fn muse_code_config_follows_an_absolute_xdg_config_home() {
    let home = Home::new();
    let mut env = home.env();
    env.xdg_config_home = Some(PathBuf::from("relative"));
    assert_eq!(
        Client::MuseCode.file(&env).expect("Muse path"),
        home.0.join(".config/muse/settings.json")
    );
    assert_eq!(Client::MuseCode.source(&env), "HOME");
    env.home = None;
    env.xdg_config_home = Some(PathBuf::from("/profiles/xdg"));
    assert_eq!(
        Client::MuseCode.file(&env).expect("Muse path"),
        PathBuf::from("/profiles/xdg/muse/settings.json")
    );
    assert_eq!(Client::MuseCode.source(&env), "XDG_CONFIG_HOME");
}

#[test]
fn muse_code_is_detected_by_program_or_settings_file() {
    let home = Home::new();
    let env = home.env();
    let no_muse = |_: &str| false;
    assert!(!Client::MuseCode.installed(&env, &no_muse));
    let muse_on_path = |program: &str| program == "muse";
    assert!(Client::MuseCode.installed(&env, &muse_on_path));
    home.put(".config/muse/settings.json", "{}");
    assert!(Client::MuseCode.installed(&env, &no_muse));
}

#[test]
fn muse_code_without_a_key_writes_no_credentials() {
    let home = Home::new();
    let env = home.env();
    let file = Client::MuseCode.file(&env).expect("Muse path");
    let setup = Setup {
        env: &env,
        url: "http://localhost:7420/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: None,
        client_config: &file,
        source: "HOME",
    };
    let preview = clients::configure(Client::MuseCode, &setup, false).expect("preview");
    assert!(!preview.contains("stores the key"), "{preview}");
    clients::configure(Client::MuseCode, &setup, true).expect("write");
    let parsed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.0.join(".config/muse/settings.json")).expect("config"),
    )
    .expect("JSON");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(
        parsed["mcpServers"]["kkt"]["url"],
        "http://localhost:7420/mcp"
    );
    assert!(parsed["mcpServers"]["kkt"].get("headers").is_none());
}

#[test]
fn malformed_client_files_are_left_intact_and_errors_do_not_echo_secrets() {
    for (client, file, original) in [
        (
            Client::Codex,
            ".codex/config.toml",
            "secret = '0123456789abcdef0123456789abcdef\n",
        ),
        (
            Client::OpenCode,
            ".config/opencode/opencode.json",
            "{\"secret\":\"0123456789abcdef0123456789abcdef",
        ),
        (
            Client::MuseCode,
            ".config/muse/settings.json",
            "{\"secret\":\"0123456789abcdef0123456789abcdef",
        ),
    ] {
        let home = Home::new();
        let file = home.put(file, original);
        let env = home.env();
        let setup = Setup {
            env: &env,
            url: "http://localhost:7420/mcp",
            executable: Path::new("/opt/kkt"),
            key_file: None,
            client_config: &file,
            source: "--client-config",
        };
        let error = clients::configure(client, &setup, true).expect_err("invalid");
        assert!(!error.contains(KEY));
        assert_eq!(std::fs::read_to_string(file).expect("config"), original);
    }
}

#[test]
fn legacy_antigravity_path_is_used_when_it_is_the_existing_file() {
    let home = Home::new();
    let file = home.put(".gemini/antigravity/mcp_config.json", "{}");
    let env = home.env();
    let client_config = Client::Agy.file(&env).expect("legacy config path");
    assert_eq!(client_config, file);
    let setup = Setup {
        env: &env,
        url: "http://localhost:7420/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: None,
        client_config: &client_config,
        source: Client::Agy.source(&env),
    };
    clients::configure(Client::Agy, &setup, true).expect("write");
    assert!(
        std::fs::read_to_string(file)
            .expect("config")
            .contains("serverUrl")
    );
    assert!(!home.0.join(".gemini/config/mcp_config.json").exists());
}

#[test]
fn later_changes_keep_the_original_backup_and_save_the_previous_configuration() {
    let home = Home::new();
    let original = "{\"theme\":\"keep\"}";
    let file = home.put(".config/opencode/opencode.json", original);
    let env = home.env();
    let mut setup = Setup {
        env: &env,
        url: "http://localhost:7420/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: None,
        client_config: &file,
        source: "--client-config",
    };
    clients::configure(Client::OpenCode, &setup, true).expect("first");
    let first = std::fs::read_to_string(&file).expect("first");
    setup.url = "https://tail.example/mcp";
    clients::configure(Client::OpenCode, &setup, true).expect("second");
    assert_eq!(
        std::fs::read_to_string(file.with_file_name("opencode.json.bak")).expect("original backup"),
        original
    );
    assert_eq!(
        std::fs::read_to_string(file.with_file_name("opencode.json.bak.1"))
            .expect("previous backup"),
        first
    );
}

#[test]
fn opencode_refuses_key_paths_its_file_substitution_cannot_represent() {
    let home = Home::new();
    let env = home.env();
    for name in ["key{brace", "key\"quote", "key\\slash", "key\nnewline"] {
        let key = home.put(name, KEY);
        let setup = Setup {
            env: &env,
            url: "http://localhost:7420/mcp",
            executable: Path::new("/opt/kkt"),
            key_file: Some(&key),
            client_config: Path::new(".config/opencode/opencode.json"),
            source: "--client-config",
        };
        let error =
            clients::configure(Client::OpenCode, &setup, true).expect_err("unsupported reference");
        assert!(error.contains("file references cannot represent"));
        assert!(!home.0.join(".config/opencode/opencode.json").exists());
    }
}

#[test]
fn codex_accepts_inline_server_tables_without_losing_existing_entries() {
    let home = Home::new();
    let file = home.put(
        ".codex/config.toml",
        "mcp_servers = { other = { url = 'https://other/mcp' } }\n",
    );
    let env = home.env();
    let setup = Setup {
        env: &env,
        url: "http://localhost:7420/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: None,
        client_config: &file,
        source: "--client-config",
    };
    clients::configure(Client::Codex, &setup, true).expect("inline write");
    let contents = std::fs::read_to_string(file).expect("config");
    let parsed: toml::Value = toml::from_str(&contents).expect("valid TOML");
    assert_eq!(
        parsed["mcp_servers"]["other"]["url"].as_str(),
        Some("https://other/mcp")
    );
    assert_eq!(
        parsed["mcp_servers"]["kkt"]["url"].as_str(),
        Some("http://localhost:7420/mcp")
    );
}

#[test]
fn opencode_local_to_remote_conversion_removes_local_only_options() {
    let home = Home::new();
    let file = home.put(
        ".config/opencode/opencode.json",
        r#"{
        "mcp": {
            "other": {"type":"local", "command":["other"], "environment":{"KEEP":"yes"}},
            "kkt": {"type":"local", "command":["old-kkt"], "environment":{"OLD":"value"},
                    "cwd":"/old/workdir", "enabled":true, "timeout":10000}
        }
    }"#,
    );
    let env = home.env();
    let setup = Setup {
        env: &env,
        url: "http://localhost:7420/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: None,
        client_config: &file,
        source: "--client-config",
    };
    clients::configure(Client::OpenCode, &setup, true).expect("convert");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).expect("config")).expect("JSON");
    let entry = &parsed["mcp"]["kkt"];
    assert_eq!(entry["type"], "remote");
    assert_eq!(entry["url"], "http://localhost:7420/mcp");
    assert_eq!(entry["enabled"], true);
    assert_eq!(entry["timeout"], 10000);
    for name in ["command", "environment", "cwd"] {
        assert!(
            entry.get(name).is_none(),
            "local-only {name} survived conversion"
        );
    }
    assert_eq!(parsed["mcp"]["other"]["environment"]["KEEP"], "yes");
}

#[test]
fn claude_profile_path_does_not_require_home() {
    let mut env = Home::new().env();
    env.home = None;
    env.claude_config_dir = Some(PathBuf::from("/profiles/work"));
    assert_eq!(
        Client::ClaudeCode.file(&env).expect("profile path"),
        PathBuf::from("/profiles/work/.claude.json")
    );
}

#[test]
fn codex_and_opencode_follow_their_own_variables() {
    let mut env = Home::new().env();
    env.home = None;
    env.codex_home = Some(PathBuf::from("/profiles/codex"));
    env.xdg_config_home = Some(PathBuf::from("/profiles/xdg"));
    assert_eq!(
        Client::Codex.file(&env).expect("Codex path"),
        PathBuf::from("/profiles/codex/config.toml")
    );
    assert_eq!(
        Client::OpenCode.file(&env).expect("OpenCode path"),
        PathBuf::from("/profiles/xdg/opencode/opencode.json")
    );
}

#[test]
fn the_source_names_the_setting_that_chose_the_default_file() {
    let home = Home::new();
    let mut env = home.env();
    for client in Client::ALL {
        assert_eq!(client.source(&env), "HOME", "{client:?}");
    }
    env.claude_config_dir = Some(PathBuf::from("/profiles/claude"));
    env.codex_home = Some(PathBuf::from("/profiles/codex"));
    env.xdg_config_home = Some(PathBuf::from("relative"));
    assert_eq!(Client::ClaudeCode.source(&env), "CLAUDE_CONFIG_DIR");
    assert_eq!(Client::Codex.source(&env), "CODEX_HOME");
    assert_eq!(Client::OpenCode.source(&env), "HOME");
    env.xdg_config_home = Some(PathBuf::from("/profiles/xdg"));
    assert_eq!(Client::OpenCode.source(&env), "XDG_CONFIG_HOME");
    assert_eq!(Client::Agy.source(&env), "HOME");
}

#[test]
fn opencode_ignores_a_relative_xdg_config_home() {
    let home = Home::new();
    let mut env = home.env();
    env.xdg_config_home = Some(PathBuf::from("relative"));
    assert_eq!(
        Client::OpenCode.file(&env).expect("OpenCode path"),
        home.0.join(".config/opencode/opencode.json")
    );
}

#[cfg(unix)]
#[test]
fn claude_reports_a_selected_profile_directory_it_cannot_resolve() {
    let home = Home::new();
    let looped = home.0.join("loop");
    std::os::unix::fs::symlink(&looped, &looped).expect("symlink loop");
    let mut env = home.env();
    env.claude_config_dir = Some(looped.join("profile"));
    let file = Client::ClaudeCode.file(&env).expect("profile path");
    let setup = Setup {
        env: &env,
        url: "http://localhost:7420/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: None,
        client_config: &file,
        source: Client::ClaudeCode.source(&env),
    };
    let error = clients::configure(Client::ClaudeCode, &setup, false)
        .expect_err("unresolvable profile directory");
    assert!(error.contains(&file.display().to_string()), "{error}");
    assert!(error.contains("(from CLAUDE_CONFIG_DIR)"), "{error}");
}

#[cfg(unix)]
#[test]
fn claude_explicit_config_ignores_an_inherited_profile_it_cannot_resolve() {
    let home = Home::new();
    let file = home.put("work/.claude.json", "{}");
    let not_a_directory = home.put("plain-file", "");
    let mut env = home.env();
    env.claude_config_dir = Some(not_a_directory.join("profile"));
    env.home = Some(not_a_directory.join("home"));
    let setup = Setup {
        env: &env,
        url: "http://localhost:7420/mcp",
        executable: Path::new("/opt/kkt"),
        key_file: None,
        client_config: &file,
        source: "--client-config",
    };
    let preview =
        clients::configure(Client::ClaudeCode, &setup, false).expect("explicit config preview");
    let dir = file
        .parent()
        .expect("parent")
        .canonicalize()
        .expect("profile directory");
    assert!(
        preview.contains(&format!("CLAUDE_CONFIG_DIR='{}'", dir.display())),
        "{preview}"
    );
}
