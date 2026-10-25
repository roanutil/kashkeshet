//! The settings a server starts with, gathered from the command line, the
//! config file, and the defaults in `config.rs`. A flag wins over the file,
//! and the file over the default.
//!
//! This part reads no files and no environment variables itself. Callers
//! pass in the config file's text and the environment values it needs
//! ([`Env`]), so every rule here can be tested directly.

mod file;
mod paths;

pub use file::parse_file;
pub use paths::{Env, Platform};

use crate::config::{DEFAULT_BIND, Limits};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Server settings that may or may not be given, from one source: the
/// command line or the config file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeOptions {
    pub db: Option<PathBuf>,
    pub bind: Option<SocketAddr>,
    pub max_response_bytes: Option<usize>,
    pub default_limit: Option<usize>,
    pub max_limit: Option<usize>,
    pub max_body_chars: Option<usize>,
    pub sends_per_minute: Option<u32>,
    pub allow_origins: Option<Vec<String>>,
    pub allow_hosts: Option<Vec<String>>,
    pub instructions: Option<PathBuf>,
    pub access_key_file: Option<PathBuf>,
}

impl ServeOptions {
    /// Nothing given.
    pub const NONE: ServeOptions = ServeOptions {
        db: None,
        bind: None,
        max_response_bytes: None,
        default_limit: None,
        max_limit: None,
        max_body_chars: None,
        sends_per_minute: None,
        allow_origins: None,
        allow_hosts: None,
        instructions: None,
        access_key_file: None,
    };

    /// Each setting from `self` if given there, else from `fallback`. A list
    /// given in `self` replaces the one in `fallback`; the two are not joined.
    pub fn over(self, fallback: ServeOptions) -> ServeOptions {
        ServeOptions {
            db: self.db.or(fallback.db),
            bind: self.bind.or(fallback.bind),
            max_response_bytes: self.max_response_bytes.or(fallback.max_response_bytes),
            default_limit: self.default_limit.or(fallback.default_limit),
            max_limit: self.max_limit.or(fallback.max_limit),
            max_body_chars: self.max_body_chars.or(fallback.max_body_chars),
            sends_per_minute: self.sends_per_minute.or(fallback.sends_per_minute),
            allow_origins: self.allow_origins.or(fallback.allow_origins),
            allow_hosts: self.allow_hosts.or(fallback.allow_hosts),
            instructions: self.instructions.or(fallback.instructions),
            access_key_file: self.access_key_file.or(fallback.access_key_file),
        }
    }

    /// The settings to run with: what was given, and the defaults for the
    /// rest. `default_db` is used only when no database was given; the
    /// default differs between bare `kkt` and `kkt serve`.
    pub fn resolve(self, default_db: PathBuf) -> ServeSettings {
        let defaults = Limits::DEFAULT;
        ServeSettings {
            db: self.db.unwrap_or(default_db),
            bind: self.bind.unwrap_or(DEFAULT_BIND),
            limits: Limits {
                max_response_bytes: self
                    .max_response_bytes
                    .unwrap_or(defaults.max_response_bytes),
                default_limit: self.default_limit.unwrap_or(defaults.default_limit),
                max_limit: self.max_limit.unwrap_or(defaults.max_limit),
                max_body_chars: self.max_body_chars.unwrap_or(defaults.max_body_chars),
                sends_per_minute: self.sends_per_minute.unwrap_or(defaults.sends_per_minute),
                ..defaults
            },
            allow_origins: self.allow_origins.unwrap_or_default(),
            allow_hosts: self.allow_hosts.unwrap_or_default(),
            instructions: self.instructions,
            access_key_file: self.access_key_file,
        }
    }
}

/// The settings a server runs with. They are not yet checked: the caller
/// runs the checks in `config` and `hosts`, which name any bad setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeSettings {
    pub db: PathBuf,
    pub bind: SocketAddr,
    pub limits: Limits,
    pub allow_origins: Vec<String>,
    pub allow_hosts: Vec<String>,
    /// A file of instructions for agents, in place of the built-in ones.
    pub instructions: Option<PathBuf>,
    /// A file holding the access key. Without one, `/mcp` needs no key.
    pub access_key_file: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_given_means_the_defaults() {
        let settings = ServeOptions::NONE.resolve(PathBuf::from("default.db"));
        assert_eq!(
            settings,
            ServeSettings {
                db: PathBuf::from("default.db"),
                bind: DEFAULT_BIND,
                limits: Limits::DEFAULT,
                allow_origins: vec![],
                allow_hosts: vec![],
                instructions: None,
                access_key_file: None,
            }
        );
        assert_eq!(settings.bind.to_string(), "127.0.0.1:7420");
    }

    #[test]
    fn a_given_setting_wins_over_the_fallback() {
        let flags = ServeOptions {
            bind: Some("127.0.0.1:9000".parse().expect("addr")),
            allow_hosts: Some(vec!["flag.example".into()]),
            ..ServeOptions::NONE
        };
        let file = ServeOptions {
            db: Some(PathBuf::from("/data/file.db")),
            bind: Some("127.0.0.1:8000".parse().expect("addr")),
            max_limit: Some(50),
            allow_hosts: Some(vec!["file.example".into(), "other.example".into()]),
            ..ServeOptions::NONE
        };
        let settings = flags.over(file).resolve(PathBuf::from("default.db"));
        assert_eq!(settings.bind.to_string(), "127.0.0.1:9000");
        assert_eq!(settings.db, PathBuf::from("/data/file.db"));
        assert_eq!(settings.limits.max_limit, 50);
        assert_eq!(settings.limits.default_limit, Limits::DEFAULT.default_limit);
        assert_eq!(settings.allow_hosts, ["flag.example"]);
    }
}
