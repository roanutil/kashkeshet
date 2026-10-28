//! The real [`Host`]: files on disk, real commands, lines on the terminal.

use super::{Command, Finished, Host};
use std::io;
use std::path::Path;
use std::process::Stdio;

pub struct OsHost;

impl Host for OsHost {
    fn run(&self, command: &Command) -> io::Result<Finished> {
        let output = process(command).stdin(Stdio::null()).output()?;
        Ok(Finished {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn run_attached(&self, command: &Command) -> io::Result<Finished> {
        let status = process(command).stdin(Stdio::null()).status()?;
        Ok(Finished {
            code: status.code(),
            ..Finished::default()
        })
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_file(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn write_file(&self, path: &Path, contents: &str, private: bool) -> io::Result<()> {
        write(path, contents, private)
    }

    fn say(&self, line: &str) {
        println!("{line}");
    }
}

/// The process for `command`: in its folder, without the variables it must
/// not see.
fn process(command: &Command) -> std::process::Command {
    let mut process = std::process::Command::new(command.program);
    process.args(&command.args);
    for name in &command.unset {
        process.env_remove(name);
    }
    if let Some(dir) = &command.current_dir {
        process.current_dir(dir);
    }
    process
}

#[cfg(unix)]
fn write(path: &Path, contents: &str, private: bool) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mode = crate::config::ACCESS_KEY_FILE_MODE;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if private {
        // New files start private, so there is no moment when they are not.
        options.mode(mode);
    }
    let mut file = options.open(path)?;
    if private {
        // A file that was already there keeps its old mode when opened.
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    file.write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write(path: &Path, contents: &str, _private: bool) -> io::Result<()> {
    std::fs::write(path, contents)
}
