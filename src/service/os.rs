//! The [`System`] implementations: the real one, and a dry run that reports
//! changes instead of making them.

use super::{Command, Finished, System};
use std::cell::RefCell;
use std::io;
use std::path::Path;
use std::process::Stdio;

/// The real system: files on disk and real commands.
pub struct OsSystem;

impl System for OsSystem {
    fn user_id(&self) -> io::Result<u32> {
        let finished = self.run(&Command::query("id", &["-u"]))?;
        finished
            .stdout
            .trim()
            .parse()
            .map_err(|_| io::Error::other(format!("id -u printed {:?}", finished.stdout)))
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn write_file(&self, path: &Path, contents: &str) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn run(&self, command: &Command) -> io::Result<Finished> {
        let output = std::process::Command::new(command.program)
            .args(&command.args)
            .stdin(Stdio::null())
            .output()?;
        Ok(Finished {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn run_attached(&self, command: &Command) -> io::Result<Finished> {
        let status = std::process::Command::new(command.program)
            .args(&command.args)
            .stdin(Stdio::null())
            .status()?;
        Ok(Finished {
            code: status.code(),
            ..Finished::default()
        })
    }
}

/// Looks at the real system but changes nothing: each change, and each
/// command that changes something, is written down instead and taken as
/// succeeding. Queries still run. Files written are not then seen to exist.
pub struct DryRun<'a> {
    real: &'a dyn System,
    actions: RefCell<Vec<String>>,
}

impl<'a> DryRun<'a> {
    /// A dry run that answers questions from `real`.
    pub fn new(real: &'a dyn System) -> DryRun<'a> {
        DryRun {
            real,
            actions: RefCell::new(Vec::new()),
        }
    }

    /// What would have been done, in order.
    pub fn actions(self) -> Vec<String> {
        self.actions.into_inner()
    }

    fn note(&self, action: String) {
        self.actions.borrow_mut().push(action);
    }
}

impl System for DryRun<'_> {
    fn user_id(&self) -> io::Result<u32> {
        self.real.user_id()
    }

    fn exists(&self, path: &Path) -> bool {
        self.real.exists(path)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        if !self.real.exists(path) {
            self.note(format!("Would create {}", path.display()));
        }
        Ok(())
    }

    fn write_file(&self, path: &Path, contents: &str) -> io::Result<()> {
        self.note(format!("Would write {}:\n{contents}", path.display()));
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.note(format!("Would remove {}", path.display()));
        Ok(())
    }

    fn run(&self, command: &Command) -> io::Result<Finished> {
        if command.query {
            return self.real.run(command);
        }
        self.note(format!("Would run: {command}"));
        Ok(Finished {
            code: Some(0),
            ..Finished::default()
        })
    }

    fn run_attached(&self, command: &Command) -> io::Result<Finished> {
        self.run(command)
    }
}
