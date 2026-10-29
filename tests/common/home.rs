//! Give each program a test runs a home directory of its own, so it never
//! reads or writes a real config file, key or client profile.

use std::path::Path;

/// Variables that point kkt, or a client it configures, at files outside
/// HOME. A test that needs one sets it after `own_home`.
const OUTSIDE_HOME: [&str; 4] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

pub trait OwnHome {
    /// Set HOME to `home` and unset every variable that could lead outside it.
    fn own_home(&mut self, home: impl AsRef<Path>) -> &mut Self;
}

impl OwnHome for std::process::Command {
    fn own_home(&mut self, home: impl AsRef<Path>) -> &mut Self {
        self.env("HOME", home.as_ref());
        for name in OUTSIDE_HOME {
            self.env_remove(name);
        }
        self
    }
}

impl OwnHome for tokio::process::Command {
    fn own_home(&mut self, home: impl AsRef<Path>) -> &mut Self {
        self.env("HOME", home.as_ref());
        for name in OUTSIDE_HOME {
            self.env_remove(name);
        }
        self
    }
}
