//! Client configuration and its on-disk formats. Helpers refer to the key
//! file; only Antigravity and Muse Code require a copy of the key in their
//! configuration.

mod files;
mod formats;

use crate::config;
use crate::settings::Env;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    ClaudeCode,
    Codex,
    Agy,
    OpenCode,
    MuseCode,
}

/// Inputs chosen by the caller, including the absolute executable/key paths.
pub struct Setup<'a> {
    pub env: &'a Env,
    pub url: &'a str,
    pub executable: &'a Path,
    pub key_file: Option<&'a Path>,
    pub client_config: &'a Path,
    /// Where `client_config` came from, for errors: `--client-config`, or
    /// what [`Client::source`] names.
    pub source: &'a str,
}

impl Client {
    pub const ALL: [Client; 5] = [
        Client::ClaudeCode,
        Client::Codex,
        Client::OpenCode,
        Client::Agy,
        Client::MuseCode,
    ];

    /// The name `kkt connect` and `kkt setup --clients` take.
    pub fn name(self) -> &'static str {
        match self {
            Client::ClaudeCode => "claude-code",
            Client::Codex => "codex",
            Client::Agy => "agy",
            Client::OpenCode => "opencode",
            Client::MuseCode => "muse-code",
        }
    }

    pub fn from_name(name: &str) -> Option<Client> {
        Client::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The client's name for people.
    pub fn display_name(self) -> &'static str {
        match self {
            Client::ClaudeCode => "Claude Code",
            Client::Codex => "Codex",
            Client::Agy => "Antigravity",
            Client::OpenCode => "OpenCode",
            Client::MuseCode => "Muse Code",
        }
    }

    /// Whether this client has no way to read the key from its file, so
    /// `configure` copies the key into the config and warns about it.
    pub fn copies_key(self) -> bool {
        matches!(self, Client::Agy | Client::MuseCode)
    }

    /// Whether the client looks installed: its config file exists, or its
    /// program is found by `on_path`.
    pub fn installed(self, env: &Env, on_path: &dyn Fn(&str) -> bool) -> bool {
        let program = match self {
            Client::ClaudeCode => Some(config::CLAUDE_PROGRAM),
            Client::Codex => Some(config::CODEX_PROGRAM),
            Client::OpenCode => Some(config::OPENCODE_PROGRAM),
            Client::MuseCode => Some(config::MUSE_PROGRAM),
            // Antigravity is an editor; its config file is the sign.
            Client::Agy => None,
        };
        program.is_some_and(on_path) || self.file(env).is_ok_and(|path| path.exists())
    }

    /// The setting that chose [`Client::file`]: the client's own variable
    /// when it is set, otherwise HOME.
    pub fn source(self, env: &Env) -> &'static str {
        match self {
            Self::ClaudeCode if env.claude_config_dir.is_some() => "CLAUDE_CONFIG_DIR",
            Self::Codex if env.codex_home.is_some() => "CODEX_HOME",
            Self::OpenCode | Self::MuseCode
                if env
                    .xdg_config_home
                    .as_ref()
                    .is_some_and(|p| p.is_absolute()) =>
            {
                "XDG_CONFIG_HOME"
            }
            _ => "HOME",
        }
    }

    /// The file name the client requires, when it reads only that name.
    fn required_name(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode => Some(config::CLAUDE_CONFIG),
            Self::Codex => Some(config::CODEX_CONFIG),
            Self::MuseCode => Some(config::MUSE_CONFIG),
            Self::Agy | Self::OpenCode => None,
        }
    }

    /// Refuse a config file the client would never read because of its name.
    /// `source` is the setting that chose `path`.
    pub fn check_name(self, path: &Path, source: &str) -> Result<(), String> {
        match self.required_name() {
            Some(name) if path.file_name() != Some(std::ffi::OsStr::new(name)) => Err(format!(
                "{} config must be named {name}: {} (from {source})",
                self.display_name(),
                path.display()
            )),
            _ => Ok(()),
        }
    }

    /// The config file the client reads by default, following the client's
    /// own variable (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_CONFIG_HOME`)
    /// when it is set.
    pub fn file(self, env: &Env) -> Result<PathBuf, String> {
        let home = || {
            env.home
                .as_ref()
                .ok_or("set HOME to find the client's config")
        };
        Ok(match self {
            Self::ClaudeCode => match &env.claude_config_dir {
                Some(dir) => dir.join(config::CLAUDE_CONFIG),
                None => home()?.join(config::CLAUDE_CONFIG),
            },
            Self::Codex => match &env.codex_home {
                Some(dir) => dir.join(config::CODEX_CONFIG),
                None => home()?
                    .join(config::CODEX_HOME_DEFAULT)
                    .join(config::CODEX_CONFIG),
            },
            Self::Agy => {
                let home = home()?;
                if !home.join(config::AGY_CONFIG).exists()
                    && home.join(config::AGY_LEGACY_CONFIG).exists()
                {
                    home.join(config::AGY_LEGACY_CONFIG)
                } else {
                    home.join(config::AGY_CONFIG)
                }
            }
            Self::OpenCode => env
                .config_home()
                .ok_or("set HOME to find the client's config")?
                .join(config::OPENCODE_CONFIG),
            Self::MuseCode => env
                .config_home()
                .ok_or("set HOME to find the client's config")?
                .join(config::MUSE_CONFIG_DIR)
                .join(config::MUSE_CONFIG),
        })
    }
}

/// A shell word, for client helper commands (not for running a process here).
pub fn shell_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The directory to pass as CLAUDE_CONFIG_DIR, or None to unset it.
/// Preserve a matching inherited profile; otherwise use Claude's normal
/// settings root for the default config and the parent for a custom config.
pub(super) fn claude_config_dir(setup: &Setup<'_>) -> Result<Option<PathBuf>, String> {
    let directory = |path: &Path| {
        let path = if path.as_os_str().is_empty() {
            Path::new(".")
        } else {
            path
        };
        let absolute = std::path::absolute(path)?;
        // HOME and the current directory can name the same directory through
        // different symlinks. Missing profile directories still use absolute paths.
        match absolute.canonicalize() {
            Ok(real) => Ok(real),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(absolute),
            Err(e) => Err(e),
        }
    };
    let unresolved = |problem: String| {
        format!(
            "could not resolve the directory of the Claude Code config {} (from {}): {problem}",
            setup.client_config.display(),
            setup.source
        )
    };
    let parent = setup
        .client_config
        .parent()
        .ok_or_else(|| unresolved("it has no parent directory".into()))?;
    let selected = directory(parent).map_err(|e| unresolved(e.to_string()))?;
    // Only the selected directory must resolve: an inherited profile or HOME
    // that cannot be resolved is not the selected one.
    let selects = |path: &PathBuf| directory(path).is_ok_and(|dir| dir == selected);
    if setup.env.claude_config_dir.as_ref().is_some_and(selects) {
        return Ok(Some(selected));
    }
    if setup.env.home.as_ref().is_some_and(selects) {
        return Ok(None);
    }
    Ok(Some(selected))
}

pub(crate) fn helper(setup: &Setup<'_>) -> Result<Option<String>, String> {
    let Some(key) = setup.key_file else {
        return Ok(None);
    };
    let executable = setup.executable.to_str().ok_or("kkt's path is not UTF-8")?;
    let key = key.to_str().ok_or("the key path is not UTF-8")?;
    Ok(Some(format!(
        "{} key header --json --key-file {}",
        shell_word(executable),
        shell_word(key)
    )))
}

/// The requested plain header, or the JSON header map both helper clients use.
pub fn header(key: &str, json: bool) -> Result<String, String> {
    config::check_access_key(key).map_err(|e| e.to_string())?;
    if json {
        formats::header_json(key)
    } else {
        Ok(format!("Authorization: Bearer {key}"))
    }
}

/// Print a plan or apply one change. A plan never contains the key, even for
/// clients that copy it; --write reads it directly from the key file instead.
pub fn configure(client: Client, setup: &Setup<'_>, write: bool) -> Result<String, String> {
    client.check_name(setup.client_config, setup.source)?;
    let claude_config_dir = match client {
        Client::ClaudeCode => claude_config_dir(setup)?,
        _ => None,
    };
    // File errors name the setting that chose the path.
    let from = |error: String| format!("{error} (from {})", setup.source);
    let original = files::read(setup.client_config).map_err(from)?;
    if !write {
        let entry = formats::preview(client, setup, claude_config_dir.as_deref())?;
        let mut out = format!("Config: {}\n{entry}\n", setup.client_config.display());
        if client.copies_key() && setup.key_file.is_some() {
            out.push_str(&format!(
                "{} stores the key in this file. --write reads the key file and sets permissions to 0600; the preview redacts it.\n",
                client.display_name()
            ));
        }
        out.push_str("Run again with --write to apply this configuration.\n");
        return Ok(out);
    }
    let updated = formats::merge(client, setup, original.as_deref()).map_err(|error| {
        format!(
            "{} (from {}): {error}",
            setup.client_config.display(),
            setup.source
        )
    })?;
    if updated.is_none() {
        if client.copies_key() {
            files::restrict(setup.client_config).map_err(from)?;
        }
        return Ok(format!(
            "Already configured: {}",
            setup.client_config.display()
        ));
    }
    let updated = updated.ok_or("missing merged client configuration")?;
    if client == Client::ClaudeCode {
        let absolute = std::path::absolute(setup.client_config).map_err(|e| {
            format!(
                "could not resolve the Claude Code config {} (from {}): {e}",
                setup.client_config.display(),
                setup.source
            )
        })?;
        files::apply_claude(
            &absolute,
            original.as_deref(),
            &updated,
            claude_config_dir.as_deref(),
            setup.source,
        )?;
    } else {
        files::replace(setup.client_config, original.as_deref(), &updated).map_err(from)?;
    }
    let mut out = format!("Configured: {}", setup.client_config.display());
    if client.copies_key() && setup.key_file.is_some() {
        out.push_str(&format!(
            "\n{} stores the key in this file; permissions are 0600.",
            client.display_name()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_back() {
        for client in Client::ALL {
            assert_eq!(Client::from_name(client.name()), Some(client));
        }
        assert_eq!(Client::from_name("cursor"), None);
    }

    #[test]
    fn a_client_is_found_by_its_program_or_its_config_file() {
        let home = std::env::temp_dir().join(format!("kashkeshet-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let codex = home.join(config::CODEX_HOME_DEFAULT);
        std::fs::create_dir_all(&codex).expect("home");
        std::fs::write(codex.join(config::CODEX_CONFIG), "").expect("codex config");
        let env = Env {
            home: Some(home.clone()),
            xdg_config_home: None,
            xdg_data_home: None,
            claude_config_dir: None,
            codex_home: None,
        };
        let on_path = |program: &str| program == config::CLAUDE_PROGRAM;
        let found: Vec<Client> = Client::ALL
            .into_iter()
            .filter(|c| c.installed(&env, &on_path))
            .collect();
        assert_eq!(found, [Client::ClaudeCode, Client::Codex]);
        std::fs::remove_dir_all(&home).expect("cleanup");
    }
}
