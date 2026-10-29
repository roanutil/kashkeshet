//! The settings a server starts with, gathered from the command line, the
//! config file, and the defaults in `config.rs`. A flag wins over the file,
//! and the file over the default.
//!
//! This part reads no files and no environment variables itself. Callers
//! pass in the config file's text and the environment values it needs
//! ([`Env`]), so every rule here can be tested directly.

mod file;
mod paths;

pub use file::{parse_file, with_allowed_host};
pub use paths::{Env, Platform};

use crate::config::{
    DEFAULT_ALLOW_ANY_BIND, DEFAULT_BIND, DEFAULT_DEPENDENCY_LOG_LEVEL, DEFAULT_LOG_LEVEL,
    InvalidSetting, Limits, LogLevel,
};
use std::net::SocketAddr;
use std::path::PathBuf;

/// Server settings that may or may not be given, from one source: the
/// command line or the config file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeOptions {
    pub db: Option<PathBuf>,
    pub bind: Option<SocketAddr>,
    /// Allow `bind` to be a non-loopback address. Needs an access key.
    pub allow_any_bind: Option<bool>,
    pub max_response_bytes: Option<usize>,
    pub default_limit: Option<usize>,
    pub max_limit: Option<usize>,
    pub sends_per_minute: Option<u32>,
    pub waiting_grace_seconds: Option<u32>,
    /// How many backups made before upgrading the database to keep.
    pub upgrade_backups_kept: Option<usize>,
    pub allow_origins: Option<Vec<String>>,
    pub allow_hosts: Option<Vec<String>>,
    pub instructions: Option<PathBuf>,
    pub access_key_file: Option<PathBuf>,
    /// Run with no access key. Refused together with `access_key_file`.
    pub no_access_key: Option<bool>,
    /// Write the log to this file, kept bounded, instead of to stdout.
    pub log_file: Option<PathBuf>,
    /// How much kkt's own code logs.
    pub log_level: Option<LogLevel>,
    /// How much the libraries kkt uses log.
    pub dependency_log_level: Option<LogLevel>,
}

impl ServeOptions {
    /// Nothing given.
    pub const NONE: ServeOptions = ServeOptions {
        db: None,
        bind: None,
        allow_any_bind: None,
        max_response_bytes: None,
        default_limit: None,
        max_limit: None,
        sends_per_minute: None,
        waiting_grace_seconds: None,
        upgrade_backups_kept: None,
        allow_origins: None,
        allow_hosts: None,
        instructions: None,
        access_key_file: None,
        no_access_key: None,
        log_file: None,
        log_level: None,
        dependency_log_level: None,
    };

    /// Each setting from `self` if given there, else from `fallback`. A list
    /// given in `self` replaces the one in `fallback`; the two are not joined.
    pub fn over(self, fallback: ServeOptions) -> ServeOptions {
        ServeOptions {
            db: self.db.or(fallback.db),
            bind: self.bind.or(fallback.bind),
            allow_any_bind: self.allow_any_bind.or(fallback.allow_any_bind),
            max_response_bytes: self.max_response_bytes.or(fallback.max_response_bytes),
            default_limit: self.default_limit.or(fallback.default_limit),
            max_limit: self.max_limit.or(fallback.max_limit),
            sends_per_minute: self.sends_per_minute.or(fallback.sends_per_minute),
            waiting_grace_seconds: self
                .waiting_grace_seconds
                .or(fallback.waiting_grace_seconds),
            upgrade_backups_kept: self.upgrade_backups_kept.or(fallback.upgrade_backups_kept),
            allow_origins: self.allow_origins.or(fallback.allow_origins),
            allow_hosts: self.allow_hosts.or(fallback.allow_hosts),
            instructions: self.instructions.or(fallback.instructions),
            access_key_file: self.access_key_file.or(fallback.access_key_file),
            no_access_key: self.no_access_key.or(fallback.no_access_key),
            log_file: self.log_file.or(fallback.log_file),
            log_level: self.log_level.or(fallback.log_level),
            dependency_log_level: self.dependency_log_level.or(fallback.dependency_log_level),
        }
    }

    /// Whether the setting `name`, as an `InvalidSetting` names it, is given.
    pub fn gives(&self, name: &str) -> bool {
        match name {
            "db" => self.db.is_some(),
            "bind" => self.bind.is_some(),
            "allow_any_bind" => self.allow_any_bind.is_some(),
            "max_response_bytes" => self.max_response_bytes.is_some(),
            "default_limit" => self.default_limit.is_some(),
            "max_limit" => self.max_limit.is_some(),
            "sends_per_minute" => self.sends_per_minute.is_some(),
            "waiting_grace" => self.waiting_grace_seconds.is_some(),
            "upgrade_backups_kept" => self.upgrade_backups_kept.is_some(),
            "allow_origin" | "allow_origins" => self.allow_origins.is_some(),
            "allow_host" | "allow_hosts" => self.allow_hosts.is_some(),
            "instructions" => self.instructions.is_some(),
            "access_key_file" => self.access_key_file.is_some(),
            "no_access_key" => self.no_access_key.is_some(),
            "log_file" => self.log_file.is_some(),
            "log_level" => self.log_level.is_some(),
            "dependency_log_level" => self.dependency_log_level.is_some(),
            _ => false,
        }
    }

    /// The settings to run with: what was given, and the defaults for the
    /// rest. `default_db` is used only when no database was given.
    /// `default_key` is the key file used, and created if missing, when
    /// neither `access_key_file` nor `no_access_key` is given; `None` when
    /// there is no home directory to put it in.
    pub fn resolve(
        self,
        default_db: PathBuf,
        default_key: Option<PathBuf>,
    ) -> Result<ServeSettings, InvalidSetting> {
        let access_key = match (self.no_access_key == Some(true), self.access_key_file) {
            (true, Some(_)) => {
                return Err(InvalidSetting::new(
                    "no_access_key",
                    "cannot be set together with access_key_file",
                ));
            }
            (true, None) => AccessKey::None,
            (false, Some(path)) => AccessKey::File(path),
            (false, None) => match default_key {
                Some(path) => AccessKey::Default(path),
                None => {
                    return Err(InvalidSetting::new(
                        "access_key_file",
                        "has no default without a home directory: set HOME, \
                         or give access_key_file or no_access_key",
                    ));
                }
            },
        };
        let web_key = match &access_key {
            AccessKey::None => AccessKey::None,
            AccessKey::File(path) | AccessKey::Default(path) => {
                let web = web_key_path(path);
                if &web == path {
                    return Err(InvalidSetting::new(
                        "access_key_file",
                        "must not name the web key file",
                    ));
                }
                AccessKey::Default(web)
            }
        };
        let defaults = Limits::DEFAULT;
        Ok(ServeSettings {
            db: self.db.unwrap_or(default_db),
            bind: self.bind.unwrap_or(DEFAULT_BIND),
            allow_any_bind: self.allow_any_bind.unwrap_or(DEFAULT_ALLOW_ANY_BIND),
            limits: Limits {
                max_response_bytes: self
                    .max_response_bytes
                    .unwrap_or(defaults.max_response_bytes),
                default_limit: self.default_limit.unwrap_or(defaults.default_limit),
                max_limit: self.max_limit.unwrap_or(defaults.max_limit),
                sends_per_minute: self.sends_per_minute.unwrap_or(defaults.sends_per_minute),
                waiting_grace: self
                    .waiting_grace_seconds
                    .map(|s| std::time::Duration::from_secs(u64::from(s)))
                    .unwrap_or(defaults.waiting_grace),
                upgrade_backups_kept: self
                    .upgrade_backups_kept
                    .unwrap_or(defaults.upgrade_backups_kept),
                ..defaults
            },
            allow_origins: self.allow_origins.unwrap_or_default(),
            allow_hosts: self.allow_hosts.unwrap_or_default(),
            instructions: self.instructions,
            access_key,
            web_key,
            log_file: self.log_file,
            log_level: self.log_level.unwrap_or(DEFAULT_LOG_LEVEL),
            dependency_log_level: self
                .dependency_log_level
                .unwrap_or(DEFAULT_DEPENDENCY_LOG_LEVEL),
        })
    }
}

/// The operator web key always lives beside the server's access key.
pub fn web_key_path(access_key: &std::path::Path) -> PathBuf {
    access_key.with_file_name(crate::config::WEB_KEY_FILE_NAME)
}

/// Which access key a server uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessKey {
    /// None: `/mcp` and the web page are open to anyone who can reach them.
    None,
    /// The key in this file, which must exist.
    File(PathBuf),
    /// The key in this default file, created if it does not exist yet.
    Default(PathBuf),
}

/// The settings a server runs with. They are not yet checked: the caller
/// runs the checks in `config` and `hosts`, which name any bad setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeSettings {
    pub db: PathBuf,
    pub bind: SocketAddr,
    /// Whether `bind` may be a non-loopback address.
    pub allow_any_bind: bool,
    pub limits: Limits,
    pub allow_origins: Vec<String>,
    pub allow_hosts: Vec<String>,
    /// A file of instructions for agents, in place of the built-in ones.
    pub instructions: Option<PathBuf>,
    pub access_key: AccessKey,
    pub web_key: AccessKey,
    /// The log file, if the log goes to one instead of stdout.
    pub log_file: Option<PathBuf>,
    pub log_level: LogLevel,
    pub dependency_log_level: LogLevel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_given_means_the_defaults() {
        let settings = ServeOptions::NONE
            .resolve(
                PathBuf::from("default.db"),
                Some(PathBuf::from("default-key")),
            )
            .expect("valid");
        assert_eq!(
            settings,
            ServeSettings {
                db: PathBuf::from("default.db"),
                bind: DEFAULT_BIND,
                allow_any_bind: false,
                limits: Limits::DEFAULT,
                allow_origins: vec![],
                allow_hosts: vec![],
                instructions: None,
                access_key: AccessKey::Default(PathBuf::from("default-key")),
                web_key: AccessKey::Default(PathBuf::from("web-key")),
                log_file: None,
                log_level: LogLevel::Info,
                dependency_log_level: LogLevel::Warn,
            }
        );
        assert_eq!(settings.bind.to_string(), "127.0.0.1:7420");
    }

    #[test]
    fn gives_answers_by_the_names_checks_use() {
        let options = ServeOptions {
            max_limit: Some(1),
            allow_hosts: Some(vec![]),
            ..ServeOptions::NONE
        };
        assert!(options.gives("max_limit"));
        assert!(options.gives("allow_host"));
        assert!(!options.gives("default_limit"));
        assert!(!options.gives("max_wait"));
        assert!(!ServeOptions::NONE.gives("bind"));
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
            no_access_key: Some(true),
            ..ServeOptions::NONE
        };
        let settings = flags
            .over(file)
            .resolve(PathBuf::from("default.db"), None)
            .expect("valid");
        assert_eq!(settings.bind.to_string(), "127.0.0.1:9000");
        assert_eq!(settings.db, PathBuf::from("/data/file.db"));
        assert_eq!(settings.limits.max_limit, 50);
        assert_eq!(settings.limits.default_limit, Limits::DEFAULT.default_limit);
        assert_eq!(settings.allow_hosts, ["flag.example"]);
    }

    #[test]
    fn the_access_key_is_on_unless_turned_off() {
        let default_key = || Some(PathBuf::from("/home/sam/.config/kkt/access-key"));
        let resolve = |options: ServeOptions| options.resolve(PathBuf::from("db"), default_key());
        assert_eq!(
            resolve(ServeOptions::NONE).expect("valid").access_key,
            AccessKey::Default(PathBuf::from("/home/sam/.config/kkt/access-key"))
        );
        let file = ServeOptions {
            access_key_file: Some(PathBuf::from("/keys/k")),
            ..ServeOptions::NONE
        };
        assert_eq!(
            resolve(file.clone()).expect("valid").access_key,
            AccessKey::File(PathBuf::from("/keys/k"))
        );
        let off = ServeOptions {
            no_access_key: Some(true),
            ..ServeOptions::NONE
        };
        assert_eq!(
            resolve(off.clone()).expect("valid").access_key,
            AccessKey::None
        );
        // `no_access_key = false` is the same as not saying.
        let on = ServeOptions {
            no_access_key: Some(false),
            ..ServeOptions::NONE
        };
        assert!(matches!(
            resolve(on).expect("valid").access_key,
            AccessKey::Default(_)
        ));
        let both = ServeOptions {
            no_access_key: Some(true),
            ..file
        };
        assert_eq!(resolve(both).map_err(|e| e.name), Err("no_access_key"));
        // A flag's key file and the file's opt-out together are refused too.
        let file_off = off.over(ServeOptions {
            access_key_file: Some(PathBuf::from("/keys/k")),
            ..ServeOptions::NONE
        });
        assert_eq!(resolve(file_off).map_err(|e| e.name), Err("no_access_key"));
    }

    #[test]
    fn without_a_home_the_key_must_be_given_or_turned_off() {
        let err = ServeOptions::NONE
            .resolve(PathBuf::from("db"), None)
            .expect_err("no default key");
        assert_eq!(err.name, "access_key_file");
    }
}

#[cfg(test)]
mod waiting_tests {
    use super::*;
    #[test]
    fn grace_is_configurable_with_flag_precedence_and_checked() {
        let env = Env {
            home: Some(PathBuf::from("/tmp")),
            xdg_config_home: None,
            xdg_data_home: None,
        };
        let file = parse_file(
            "waiting_grace_seconds = 4",
            std::path::Path::new("/tmp"),
            &env,
        )
        .unwrap();
        let flags = ServeOptions {
            waiting_grace_seconds: Some(2),
            ..ServeOptions::NONE
        };
        let settings = flags
            .over(file)
            .resolve(PathBuf::from("db"), Some(PathBuf::from("key")))
            .unwrap();
        assert_eq!(
            settings.limits.waiting_grace,
            std::time::Duration::from_secs(2)
        );
        settings.limits.check().unwrap();
        let bad = Limits {
            waiting_grace: std::time::Duration::from_secs(51),
            ..Limits::DEFAULT
        };
        assert_eq!(bad.check().unwrap_err().name, "waiting_grace");
        Limits {
            waiting_grace: std::time::Duration::ZERO,
            ..Limits::DEFAULT
        }
        .check()
        .unwrap();
    }
}
