//! Backups: a folder holding a consistent copy of the database, the config
//! file and both keys, named so that backups sort by time and say which
//! kkt version wrote them; and pruning the backups the server keeps before it
//! upgrades a database, to the newest few.
//!
//! This part decides names, folders, file modes and pruning. The caller gives
//! it the time, the version, and the function that copies the database (the
//! store's online backup).

use crate::config::{ACCESS_KEY_FILE_MODE, BACKUP_FOLDER_MODE};
use chrono::{DateTime, Utc};
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The files in a backup folder.
pub const DATABASE_FILE: &str = "kashkeshet.db";
pub const CONFIG_FILE: &str = "config.toml";
pub const ACCESS_KEY_FILE: &str = "access-key";
pub const WEB_KEY_FILE: &str = "web-key";
/// Says when the backup was made, by which version, and where each file came
/// from, for restoring it.
pub const ABOUT_FILE: &str = "about.txt";

/// Every backup's name starts with this.
const PREFIX: &str = "kkt-";
/// A backup's folder has this name added while it is being written.
const PARTIAL: &str = ".partial";
/// The time in a backup's name, such as `20261028T131500.123Z`. It sorts
/// as text in time order.
const TIME_FORMAT: &str = "%Y%m%dT%H%M%S%.3fZ";

/// What to back up: the database file, and the config and key files
/// if the server has them.
pub struct Contents<'a> {
    pub database: &'a Path,
    pub config: Option<&'a Path>,
    pub access_key: Option<&'a Path>,
    pub web_key: Option<&'a Path>,
}

/// The name of a backup made at `at` by kkt `version`, such as
/// `kkt-20261028T131500.123Z-v0.3.0`.
pub fn name(at: DateTime<Utc>, version: &str) -> String {
    format!("{PREFIX}{}-v{version}", at.format(TIME_FORMAT))
}

/// Whether `name` is a finished backup's name.
fn is_backup(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(PREFIX) else {
        return false;
    };
    let Some((time, version)) = rest.split_once("-v") else {
        return false;
    };
    !version.is_empty()
        && !name.ends_with(PARTIAL)
        && chrono::NaiveDateTime::parse_from_str(time, TIME_FORMAT).is_ok()
}

/// Writes a backup of `contents` made at `at` by kkt `version` to a new
/// folder in `folder`, creating `folder` if needed, and returns the new
/// folder. `copy_database(from, to)` writes a consistent copy of the database
/// to the new file `to`.
///
/// The backup folder and the access key in it are readable only by their
/// owner. The folder is written under a temporary name and renamed when
/// complete, so a backup folder is never half written; on failure the
/// temporary folder is removed.
pub fn write(
    folder: &Path,
    at: DateTime<Utc>,
    version: &str,
    contents: &Contents,
    copy_database: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder)
        .map_err(|e| format!("could not create {}: {e}", folder.display()))?;
    let name = name(at, version);
    let done = folder.join(&name);
    if done.exists() {
        return Err(format!("{} already exists", done.display()));
    }
    let partial = folder.join(format!("{name}{PARTIAL}"));
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(BACKUP_FOLDER_MODE);
    }
    builder
        .create(&partial)
        .map_err(|e| format!("could not create {}: {e}", partial.display()))?;
    match fill(&partial, at, version, contents, copy_database) {
        Ok(()) => std::fs::rename(&partial, &done)
            .map_err(|e| format!("could not rename {}: {e}", partial.display()))
            .map(|()| done),
        Err(e) => Err(match std::fs::remove_dir_all(&partial) {
            Ok(()) => e,
            Err(removing) => format!(
                "{e}; could not remove the unfinished {}: {removing}",
                partial.display()
            ),
        }),
    }
}

fn fill(
    into: &Path,
    at: DateTime<Utc>,
    version: &str,
    contents: &Contents,
    copy_database: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    copy_database(contents.database, &into.join(DATABASE_FILE)).map_err(|e| {
        format!(
            "could not copy the database {}: {e}",
            contents.database.display()
        )
    })?;
    if let Some(config) = contents.config {
        let to = into.join(CONFIG_FILE);
        std::fs::copy(config, &to)
            .map_err(|e| format!("could not copy {}: {e}", config.display()))?;
    }
    if let Some(key) = contents.access_key {
        copy_private(key, &into.join(ACCESS_KEY_FILE))?;
    }
    if let Some(key) = contents.web_key {
        copy_private(key, &into.join(WEB_KEY_FILE))?;
    }
    let source = |path: Option<&Path>| match path {
        Some(path) => format!("copy of {}", path.display()),
        None => "none".to_string(),
    };
    let about = format!(
        "kkt {version} backup made {}\n\
         {DATABASE_FILE}: {}\n\
         {CONFIG_FILE}: {}\n\
         {ACCESS_KEY_FILE}: {}\n\
         {WEB_KEY_FILE}: {}\n",
        crate::clock::to_text(at),
        source(Some(contents.database)),
        source(contents.config),
        source(contents.access_key),
        source(contents.web_key),
    );
    std::fs::write(into.join(ABOUT_FILE), about)
        .map_err(|e| format!("could not write {}: {e}", into.join(ABOUT_FILE).display()))
}

/// Copies `from` to the new file `to`, readable only by its owner from the
/// moment it exists.
fn copy_private(from: &Path, to: &Path) -> Result<(), String> {
    let text =
        std::fs::read(from).map_err(|e| format!("could not read {}: {e}", from.display()))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(ACCESS_KEY_FILE_MODE);
    }
    options
        .open(to)
        .and_then(|mut file| file.write_all(&text))
        .map_err(|e| format!("could not write {}: {e}", to.display()))
}

/// Removes all but the newest `keep` backups in `folder`, judged by their
/// names, and returns the ones removed. Anything else in `folder` is left
/// alone. A missing `folder` has nothing to remove.
pub fn prune(folder: &Path, keep: usize) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("could not list {}: {e}", folder.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("could not list {}: {e}", folder.display()))?;
        if let Some(name) = entry.file_name().to_str().filter(|n| is_backup(n)) {
            names.push(name.to_string());
        }
    }
    names.sort();
    let old = names.len().saturating_sub(keep);
    let mut removed = Vec::new();
    for name in &names[..old] {
        let path = folder.join(name);
        std::fs::remove_dir_all(&path)
            .map_err(|e| format!("could not remove {}: {e}", path.display()))?;
        removed.push(path);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
