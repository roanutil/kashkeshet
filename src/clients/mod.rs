//! Client configuration and its on-disk formats. Helpers refer to the key
//! file; only Antigravity requires a copy of the key in its configuration.

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
}

/// Inputs chosen by the caller, including the absolute executable/key paths.
pub struct Setup<'a> {
    pub env: &'a Env,
    pub url: &'a str,
    pub executable: &'a Path,
    pub key_file: Option<&'a Path>,
}

impl Client {
    fn file(self, env: &Env) -> Result<PathBuf, String> {
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
/// Antigravity; --write reads it directly from the key file instead.
pub fn configure(client: Client, setup: &Setup<'_>, write: bool) -> Result<String, String> {
    let path = client.file(setup.env)?;
    let original = files::read(&path)?;
    if !write {
        let entry = formats::preview(client, setup)?;
        let mut out = format!("Config: {}\n{entry}\n", path.display());
        if client == Client::Agy && setup.key_file.is_some() {
            out.push_str("Antigravity stores the key in this file. --write reads the key file and sets permissions to 0600; the preview redacts it.\n");
        }
        out.push_str("Run again with --write to apply this configuration.\n");
        return Ok(out);
    }
    let updated = formats::merge(client, setup, original.as_deref())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if updated.is_none() {
        if client == Client::Agy {
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
    if client == Client::Agy && setup.key_file.is_some() {
        out.push_str("\nAntigravity stores the key in this file; permissions are 0600.");
    }
    Ok(out)
}
