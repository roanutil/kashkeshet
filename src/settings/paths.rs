//! Where the config file, the access key and the database live by default.
//! The names are in `config.rs`; this works out the full paths from the
//! environment values the caller passes in.

use crate::config::{
    ACCESS_KEY_FILE_NAME, CONFIG_DIR, CONFIG_FILE_NAME, DB_FILE_NAME, MACOS_DATA_DIR,
    XDG_CONFIG_HOME_DEFAULT, XDG_DATA_DIR, XDG_DATA_HOME_DEFAULT,
};
use std::path::PathBuf;

/// The platform, which decides where per-user data goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    /// Linux and other Unix systems, which follow the XDG base directories.
    Xdg,
}

impl Platform {
    /// The platform this binary was built for.
    pub const CURRENT: Platform = if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Xdg
    };
}

/// The environment values that decide default paths, kkt's own and the
/// clients' it configures, so `main.rs` reads the environment in one place.
/// Unset or empty values are `None`. The XDG values are ignored unless
/// absolute, as the XDG base directory specification says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Env {
    /// `$HOME`.
    pub home: Option<PathBuf>,
    /// `$CLAUDE_CONFIG_DIR`.
    pub claude_config_dir: Option<PathBuf>,
    /// `$CODEX_HOME`.
    pub codex_home: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<PathBuf>,
    /// `$XDG_DATA_HOME`.
    pub xdg_data_home: Option<PathBuf>,
}

impl Env {
    /// The config directory: `$XDG_CONFIG_HOME/kkt`, or `~/.config/kkt`. The
    /// same on every platform.
    pub fn config_dir(&self) -> Option<PathBuf> {
        Some(self.config_home()?.join(CONFIG_DIR))
    }

    /// The base of per-user config: `$XDG_CONFIG_HOME`, or `~/.config`.
    /// Other programs' config lives here too, such as systemd user units.
    pub fn config_home(&self) -> Option<PathBuf> {
        match absolute(&self.xdg_config_home) {
            Some(dir) => Some(dir),
            None => Some(self.home.as_ref()?.join(XDG_CONFIG_HOME_DEFAULT)),
        }
    }

    /// The config file `kkt serve` reads when it exists and no `--config` is given.
    pub fn config_file(&self) -> Option<PathBuf> {
        Some(self.config_dir()?.join(CONFIG_FILE_NAME))
    }

    /// Where `kkt key create` writes a key when no `--path` is given.
    pub fn access_key_file(&self) -> Option<PathBuf> {
        Some(self.config_dir()?.join(ACCESS_KEY_FILE_NAME))
    }

    /// The per-user data directory: `~/Library/Application Support/kashkeshet`
    /// on macOS, `$XDG_DATA_HOME/kashkeshet` (or `~/.local/share/kashkeshet`)
    /// elsewhere.
    pub fn data_dir(&self, platform: Platform) -> Option<PathBuf> {
        match platform {
            Platform::MacOs => Some(self.home.as_ref()?.join(MACOS_DATA_DIR)),
            Platform::Xdg => {
                let base = match absolute(&self.xdg_data_home) {
                    Some(dir) => dir,
                    None => self.home.as_ref()?.join(XDG_DATA_HOME_DEFAULT),
                };
                Some(base.join(XDG_DATA_DIR))
            }
        }
    }

    /// The database `kkt serve` uses when none is given, in [`Env::data_dir`].
    pub fn default_db(&self, platform: Platform) -> Option<PathBuf> {
        Some(self.data_dir(platform)?.join(DB_FILE_NAME))
    }
}

fn absolute(path: &Option<PathBuf>) -> Option<PathBuf> {
    path.as_ref().filter(|p| p.is_absolute()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(home: Option<&str>, config: Option<&str>, data: Option<&str>) -> Env {
        Env {
            home: home.map(PathBuf::from),
            claude_config_dir: None,
            codex_home: None,
            xdg_config_home: config.map(PathBuf::from),
            xdg_data_home: data.map(PathBuf::from),
        }
    }

    #[test]
    fn config_files_go_under_dot_config_on_every_platform() {
        let e = env(Some("/home/sam"), None, None);
        assert_eq!(
            e.config_file(),
            Some(PathBuf::from("/home/sam/.config/kkt/config.toml"))
        );
        assert_eq!(
            e.access_key_file(),
            Some(PathBuf::from("/home/sam/.config/kkt/access-key"))
        );
        let e = env(Some("/home/sam"), Some("/xdg/config"), None);
        assert_eq!(
            e.config_file(),
            Some(PathBuf::from("/xdg/config/kkt/config.toml"))
        );
    }

    #[test]
    fn the_database_goes_in_the_platform_data_directory() {
        let e = env(Some("/Users/sam"), None, None);
        assert_eq!(
            e.default_db(Platform::MacOs),
            Some(PathBuf::from(
                "/Users/sam/Library/Application Support/kashkeshet/kashkeshet.db"
            ))
        );
        let e = env(Some("/home/sam"), None, None);
        assert_eq!(
            e.default_db(Platform::Xdg),
            Some(PathBuf::from(
                "/home/sam/.local/share/kashkeshet/kashkeshet.db"
            ))
        );
        let e = env(Some("/home/sam"), None, Some("/xdg/data"));
        assert_eq!(
            e.default_db(Platform::Xdg),
            Some(PathBuf::from("/xdg/data/kashkeshet/kashkeshet.db"))
        );
    }

    #[test]
    fn relative_xdg_values_are_ignored() {
        let e = env(
            Some("/home/sam"),
            Some("relative/config"),
            Some("relative/data"),
        );
        assert_eq!(e.config_dir(), Some(PathBuf::from("/home/sam/.config/kkt")));
        assert_eq!(
            e.data_dir(Platform::Xdg),
            Some(PathBuf::from("/home/sam/.local/share/kashkeshet"))
        );
    }

    #[test]
    fn without_a_home_there_are_no_defaults_unless_xdg_says() {
        let e = env(None, None, None);
        assert_eq!(e.config_file(), None);
        assert_eq!(e.default_db(Platform::MacOs), None);
        assert_eq!(e.default_db(Platform::Xdg), None);
        let e = env(None, Some("/xdg/config"), Some("/xdg/data"));
        assert!(e.config_file().is_some());
        assert!(e.default_db(Platform::Xdg).is_some());
    }
}
