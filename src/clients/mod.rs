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

    fn file(self, env: &Env) -> Result<PathBuf, String> {
        if self == Self::MuseCode {
            // Muse Code follows the XDG config home, not just `$HOME`.
            let base = env
                .config_home()
                .ok_or("set HOME to find the client's config")?;
            return Ok(base.join(config::MUSE_CONFIG));
        }
        let home = env
            .home
            .as_ref()
            .ok_or("set HOME to find the client's config")?;
        let name = match self {
            Self::ClaudeCode => config::CLAUDE_CONFIG,
            Self::Codex => config::CODEX_CONFIG,
            Self::Agy => {
                if !home.join(config::AGY_CONFIG).exists()
                    && home.join(config::AGY_LEGACY_CONFIG).exists()
                {
                    config::AGY_LEGACY_CONFIG
                } else {
                    config::AGY_CONFIG
                }
            }
            Self::OpenCode => config::OPENCODE_CONFIG,
            Self::MuseCode => unreachable!("Muse Code returned above"),
        };
        Ok(home.join(name))
    }
}

/// A shell word, for client helper commands (not for running a process here).
pub fn shell_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
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
    let path = client.file(setup.env)?;
    let original = files::read(&path)?;
    if !write {
        let entry = formats::preview(client, setup)?;
        let mut out = format!("Config: {}\n{entry}\n", path.display());
        if client.copies_key() && setup.key_file.is_some() {
            out.push_str(&format!(
                "{} stores the key in this file. --write reads the key file and sets permissions to 0600; the preview redacts it.\n",
                client.display_name()
            ));
        }
        out.push_str("Run again with --write to apply this configuration.\n");
        return Ok(out);
    }
    let updated = formats::merge(client, setup, original.as_deref())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if updated.is_none() {
        if client.copies_key() {
            files::restrict(&path)?;
        }
        return Ok(format!("Already configured: {}", path.display()));
    }
    let updated = updated.ok_or("missing merged client configuration")?;
    if client == Client::ClaudeCode {
        files::apply_claude(&path, original.as_deref(), &updated)?;
    } else {
        files::replace(&path, original.as_deref(), &updated)?;
    }
    let mut out = format!("Configured: {}", path.display());
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
        std::fs::create_dir_all(home.join(".codex")).expect("home");
        std::fs::write(home.join(config::CODEX_CONFIG), "").expect("codex config");
        let env = Env {
            home: Some(home.clone()),
            xdg_config_home: None,
            xdg_data_home: None,
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
