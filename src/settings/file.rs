//! The config file: TOML with one key per server setting, named as the flag
//! is with `_` for `-`. The lists are `allow_origins` and `allow_hosts`.
//!
//! ```toml
//! db = "~/kkt/kashkeshet.db"
//! bind = "127.0.0.1:7420"
//! allow_hosts = ["host.docker.internal"]
//! access_key_file = "access-key"
//! # or, to run with no key at all:
//! # no_access_key = true
//! ```
//!
//! A path starting with `~/` is under the home directory; any other relative
//! path is relative to the config file's directory. An unknown key or a
//! value of the wrong kind is refused, naming the key.

use super::{Env, ServeOptions};
use crate::config::{InvalidSetting, LogLevel};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The settings in a config file's `text`. `dir` is the directory the file
/// is in, for relative paths.
pub fn parse_file(text: &str, dir: &Path, env: &Env) -> Result<ServeOptions, InvalidSetting> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        InvalidSetting::new(
            "config",
            format!("is not valid TOML: {}", e.message().trim()),
        )
    })?;
    let mut options = ServeOptions::NONE;
    for (key, value) in table {
        let value = Value(value);
        match key.as_str() {
            "db" => options.db = Some(value.path("db", dir, env)?),
            "bind" => options.bind = Some(value.address("bind")?),
            "max_response_bytes" => {
                options.max_response_bytes = Some(value.count("max_response_bytes")?)
            }
            "default_limit" => options.default_limit = Some(value.count("default_limit")?),
            "max_limit" => options.max_limit = Some(value.count("max_limit")?),
            "sends_per_minute" => options.sends_per_minute = Some(value.count("sends_per_minute")?),
            "allow_origins" => options.allow_origins = Some(value.strings("allow_origins")?),
            "allow_hosts" => options.allow_hosts = Some(value.strings("allow_hosts")?),
            "instructions" => options.instructions = Some(value.path("instructions", dir, env)?),
            "access_key_file" => {
                options.access_key_file = Some(value.path("access_key_file", dir, env)?)
            }
            "no_access_key" => options.no_access_key = Some(value.boolean("no_access_key")?),
            "log_file" => options.log_file = Some(value.path("log_file", dir, env)?),
            "log_level" => options.log_level = Some(value.log_level("log_level")?),
            "dependency_log_level" => {
                options.dependency_log_level = Some(value.log_level("dependency_log_level")?)
            }
            _ => {
                return Err(InvalidSetting::new(
                    "config",
                    format!("has an unknown setting {key:?}"),
                ));
            }
        }
    }
    Ok(options)
}

/// One value from the file.
struct Value(toml::Value);

impl Value {
    fn wrong(&self, name: &'static str, expected: &str) -> InvalidSetting {
        InvalidSetting::new(
            name,
            format!("in the config file must be {expected}, not {}", self.0),
        )
    }

    fn string(&self, name: &'static str, expected: &str) -> Result<&str, InvalidSetting> {
        self.0.as_str().ok_or_else(|| self.wrong(name, expected))
    }

    fn path(&self, name: &'static str, dir: &Path, env: &Env) -> Result<PathBuf, InvalidSetting> {
        let text = self.string(name, "a path in quotes")?;
        if text.is_empty() {
            return Err(self.wrong(name, "a path in quotes"));
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return match &env.home {
                Some(home) => Ok(home.join(rest)),
                None => Err(InvalidSetting::new(
                    name,
                    "in the config file starts with ~/, but no home directory is set",
                )),
            };
        }
        Ok(dir.join(text))
    }

    fn log_level(&self, name: &'static str) -> Result<LogLevel, InvalidSetting> {
        LogLevel::parse(
            name,
            self.string(name, "a level in quotes, such as \"info\"")?,
        )
    }

    fn boolean(&self, name: &'static str) -> Result<bool, InvalidSetting> {
        self.0
            .as_bool()
            .ok_or_else(|| self.wrong(name, "true or false"))
    }

    fn address(&self, name: &'static str) -> Result<SocketAddr, InvalidSetting> {
        let expected = "an address in quotes, such as \"127.0.0.1:7420\"";
        self.string(name, expected)?
            .parse()
            .map_err(|_| self.wrong(name, expected))
    }

    fn count<T: TryFrom<i64>>(&self, name: &'static str) -> Result<T, InvalidSetting> {
        let expected = "a whole number of zero or more";
        let n = self
            .0
            .as_integer()
            .ok_or_else(|| self.wrong(name, expected))?;
        T::try_from(n).map_err(|_| self.wrong(name, expected))
    }

    fn strings(&self, name: &'static str) -> Result<Vec<String>, InvalidSetting> {
        let expected = "a list of strings, such as [\"a\", \"b\"]";
        let items = self
            .0
            .as_array()
            .ok_or_else(|| self.wrong(name, expected))?;
        items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| self.wrong(name, expected))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            home: Some(PathBuf::from("/home/sam")),
            xdg_config_home: None,
            xdg_data_home: None,
        }
    }

    fn parse(text: &str) -> Result<ServeOptions, InvalidSetting> {
        parse_file(text, Path::new("/etc/kkt"), &env())
    }

    #[test]
    fn an_empty_file_gives_nothing() {
        assert_eq!(parse(""), Ok(ServeOptions::NONE));
    }

    #[test]
    fn every_setting_can_be_given() {
        let options = parse(
            r#"
            db = "~/data/kkt.db"
            bind = "127.0.0.1:9000"
            max_response_bytes = 4096
            default_limit = 10
            max_limit = 40
            sends_per_minute = 5
            allow_origins = ["http://localhost:6274"]
            allow_hosts = ["host.docker.internal", "box.ts.net"]
            instructions = "instructions.md"
            access_key_file = "/keys/kkt"
            no_access_key = false
            log_file = "~/logs/kkt.log"
            log_level = "debug"
            dependency_log_level = "info"
            "#,
        )
        .expect("valid file");
        assert_eq!(
            options,
            ServeOptions {
                db: Some(PathBuf::from("/home/sam/data/kkt.db")),
                bind: Some("127.0.0.1:9000".parse().expect("addr")),
                max_response_bytes: Some(4096),
                default_limit: Some(10),
                max_limit: Some(40),
                sends_per_minute: Some(5),
                allow_origins: Some(vec!["http://localhost:6274".into()]),
                allow_hosts: Some(vec!["host.docker.internal".into(), "box.ts.net".into()]),
                instructions: Some(PathBuf::from("/etc/kkt/instructions.md")),
                access_key_file: Some(PathBuf::from("/keys/kkt")),
                no_access_key: Some(false),
                log_file: Some(PathBuf::from("/home/sam/logs/kkt.log")),
                log_level: Some(LogLevel::Debug),
                dependency_log_level: Some(LogLevel::Info),
            }
        );
    }

    #[test]
    fn unknown_keys_are_refused_naming_them() {
        let err = parse("bnid = \"127.0.0.1:1\"").expect_err("unknown key");
        assert_eq!(err.name, "config");
        assert!(err.rule.contains("\"bnid\""), "{}", err.rule);
        // Flag spellings with dashes are not keys.
        let err = parse("max-limit = 5").expect_err("dashed key");
        assert!(err.rule.contains("\"max-limit\""), "{}", err.rule);
    }

    #[test]
    fn bad_values_are_refused_naming_the_setting() {
        let cases = [
            ("bind = \"not an address\"", "bind"),
            ("bind = 7420", "bind"),
            ("max_limit = -1", "max_limit"),
            ("max_limit = \"5\"", "max_limit"),
            ("sends_per_minute = 5000000000", "sends_per_minute"),
            ("allow_hosts = \"one.example\"", "allow_hosts"),
            ("allow_origins = [1]", "allow_origins"),
            ("no_access_key = \"yes\"", "no_access_key"),
            ("log_level = \"loud\"", "log_level"),
            ("dependency_log_level = 3", "dependency_log_level"),
            ("db = \"\"", "db"),
            ("access_key_file = 3", "access_key_file"),
        ];
        for (text, name) in cases {
            let err = parse(text).expect_err(text);
            assert_eq!(err.name, name, "{text}");
        }
    }

    #[test]
    fn broken_toml_is_refused() {
        let err = parse("bind = ").expect_err("broken");
        assert_eq!(err.name, "config");
    }

    #[test]
    fn home_paths_need_a_home() {
        let no_home = Env {
            home: None,
            xdg_config_home: None,
            xdg_data_home: None,
        };
        let err =
            parse_file("db = \"~/x.db\"", Path::new("/etc/kkt"), &no_home).expect_err("no home");
        assert_eq!(err.name, "db");
    }
}
