//! The `docker compose` commands, as pure functions of the managed folder.

use super::{Command, Folder};

/// `docker compose` for the managed folder, with `args` after it. The folder
/// is given outright so nothing depends on the directory kkt runs in; the
/// project name comes from the folder's settings file.
pub(super) fn compose(folder: &Folder, args: &[&str]) -> Command {
    let mut all = vec![
        "compose".to_string(),
        "--project-directory".to_string(),
        folder.path().display().to_string(),
    ];
    all.extend(args.iter().map(|a| a.to_string()));
    Command {
        current_dir: Some(folder.path().to_path_buf()),
        ..Command::docker(all)
    }
}

/// `tailscale status --json` in the Tailscale container.
pub(super) fn tailscale_status(folder: &Folder) -> Command {
    compose(
        folder,
        &["exec", "-T", "tailscale", "tailscale", "status", "--json"],
    )
}

/// `docker compose version`, which only works if Docker and its Compose
/// plugin are installed.
pub(super) fn version() -> Command {
    Command::docker(vec!["compose".into(), "version".into()])
}
