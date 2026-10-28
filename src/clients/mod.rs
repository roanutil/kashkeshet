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
    pub client_config: &'a Path,
}

impl Client {
    pub fn file(self, env: &Env) -> Result<PathBuf, String> {
        if let Self::ClaudeCode = self
            && let Some(dir) = &env.claude_config_dir
        {
            return Ok(dir.join(config::CLAUDE_CONFIG));
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
        };
        Ok(home.join(name))
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
        let absolute = std::path::absolute(path)
            .map_err(|e| format!("could not resolve client config path: {e}"))?;
        // HOME and the current directory can name the same directory through
        // different symlinks. Missing profile directories still use absolute paths.
        match absolute.canonicalize() {
            Ok(real) => Ok(real),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(absolute),
            Err(e) => Err(format!(
                "could not resolve client config path {}: {e}",
                absolute.display()
            )),
        }
    };
    let selected = directory(
        setup
            .client_config
            .parent()
            .ok_or("client config path has no parent")?,
    )?;
    if let Some(dir) = &setup.env.claude_config_dir
        && directory(dir)? == selected
    {
        return Ok(Some(selected));
    }
    if let Some(home) = &setup.env.home
        && directory(home)? == selected
    {
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
/// Antigravity; --write reads it directly from the key file instead.
pub fn configure(client: Client, setup: &Setup<'_>, write: bool) -> Result<String, String> {
    if client == Client::ClaudeCode
        && setup.client_config.file_name() != Some(std::ffi::OsStr::new(".claude.json"))
    {
        return Err(format!(
            "Claude Code config must be named .claude.json: {}",
            setup.client_config.display()
        ));
    }
    let original = files::read(setup.client_config)?;
    if !write {
        let entry = formats::preview(client, setup)?;
        let mut out = format!("Config: {}\n{entry}\n", setup.client_config.display());
        if client == Client::Agy && setup.key_file.is_some() {
            out.push_str("Antigravity stores the key in this file. --write reads the key file and sets permissions to 0600; the preview redacts it.\n");
        }
        out.push_str("Run again with --write to apply this configuration.\n");
        return Ok(out);
    }
    let updated = formats::merge(client, setup, original.as_deref())
        .map_err(|error| format!("{}: {error}", setup.client_config.display()))?;
    if updated.is_none() {
        if client == Client::Agy {
            files::restrict(setup.client_config)?;
        }
        return Ok(format!(
            "Already configured: {}",
            setup.client_config.display()
        ));
    }
    let updated = updated.ok_or("missing merged client configuration")?;
    if client == Client::ClaudeCode {
        files::apply_claude(
            setup.client_config,
            original.as_deref(),
            &updated,
            claude_config_dir(setup)?.as_deref(),
        )?;
    } else {
        files::replace(setup.client_config, original.as_deref(), &updated)?;
    }
    let mut out = format!("Configured: {}", setup.client_config.display());
    if client == Client::Agy && setup.key_file.is_some() {
        out.push_str("\nAntigravity stores the key in this file; permissions are 0600.");
    }
    Ok(out)
}
