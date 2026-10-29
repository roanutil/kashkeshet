//! Backups and atomic replacement; Claude Code writes its own user config.

use crate::config::ACCESS_KEY_FILE_MODE;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn read(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(ACCESS_KEY_FILE_MODE);
    }
    options
}

fn backup(path: &Path, text: Option<&str>) -> Result<(), String> {
    let Some(text) = text else { return Ok(()) };
    for number in 0u64.. {
        let suffix = if number == 0 {
            ".bak".to_string()
        } else {
            format!(".bak.{number}")
        };
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let name = PathBuf::from(name);
        match private_options().open(&name) {
            Ok(mut file) => {
                file.write_all(text.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|e| format!("could not back up {}: {e}", path.display()))?;
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("could not back up {}: {e}", path.display())),
        }
    }
    Err("no unused backup name".to_string())
}

pub(super) fn restrict(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(ACCESS_KEY_FILE_MODE))
            .map_err(|e| format!("could not restrict {}: {e}", path.display()))?;
    }
    Ok(())
}

fn atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("config path has no parent")?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".kkt-{}", std::process::id()));
    let temporary = PathBuf::from(name);
    let mut file = private_options()
        .open(&temporary)
        .map_err(|e| format!("could not prepare {}: {e}", path.display()))?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = written {
        fs::remove_file(&temporary).map_err(|cleanup| {
            format!(
                "could not write {}: {error}; could not remove temporary file: {cleanup}",
                path.display()
            )
        })?;
        return Err(format!("could not write {}: {error}", path.display()));
    }
    Ok(())
}

pub(super) fn replace(path: &Path, original: Option<&str>, updated: &str) -> Result<(), String> {
    backup(path, original)?;
    atomic(path, updated)
}

fn claude(args: &[&str], config_dir: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new("claude");
    command.args(args);

    if let Some(dir) = config_dir {
        command.env("CLAUDE_CONFIG_DIR", dir);
    } else {
        command.env_remove("CLAUDE_CONFIG_DIR");
    }

    let result = command.output().map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            "claude was not found on PATH; install Claude Code, then retry --write".to_string()
        } else {
            format!("could not run claude: {e}")
        }
    })?;
    if !result.status.success() {
        // The CLI may echo credentials in its output. Report only the status.
        return Err(format!("claude mcp failed with {}", result.status));
    }
    Ok(())
}

/// Apply a Claude Code change through its CLI. `client_config` is absolute;
/// `source` is the setting that chose it, named in file errors.
pub(super) fn apply_claude(
    client_config: &Path,
    original: Option<&str>,
    updated: &str,
    config_dir: Option<&Path>,
    source: &str,
) -> Result<(), String> {
    let from = |error: String| format!("{error} (from {source})");
    let entry = super::formats::claude_entry(updated)?;
    let replacing = match original {
        Some(text) => super::formats::has_claude_entry(text)?,
        None => false,
    };
    backup(client_config, original).map_err(from)?;

    let applied = (|| {
        if replacing {
            claude(&["mcp", "remove", "--scope", "user", "kkt"], config_dir)?;
        }
        claude(
            &["mcp", "add-json", "--scope", "user", "kkt", &entry],
            config_dir,
        )
    })();
    if let Err(error) = applied {
        match original {
            Some(text) => atomic(client_config, text)
                .map_err(|e| from(format!("{error}; could not restore: {e}")))?,
            None if client_config.exists() => fs::remove_file(client_config).map_err(|e| {
                from(format!(
                    "{error}; could not restore {}: {e}",
                    client_config.display()
                ))
            })?,
            None => (),
        }
        return Err(error);
    }
    Ok(())
}
