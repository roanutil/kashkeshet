//! Provenance: what produced a server run. Each run records the program
//! version, the git commit it was built from, the settings in effect, and a
//! hash of the instructions agents were given, so anything the run creates
//! can be traced back to them.

#[cfg(test)]
mod commit;

use crate::app::AppConfig;
use crate::clock;
use crate::config::Limits;
use crate::store::{Db, NewServerRunRecord, StoreResult};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

/// The build the server is running.
#[derive(Clone, Copy, Debug)]
pub struct Build {
    pub version: &'static str,
    /// The git commit, from `build.rs`. See `commit::commit_text`.
    pub commit: &'static str,
}

impl Build {
    /// This binary.
    pub const CURRENT: Build = Build {
        version: env!("CARGO_PKG_VERSION"),
        commit: env!("KASHKESHET_GIT_COMMIT"),
    };
}

/// One run of the server, from start to stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRun {
    pub version: String,
    pub commit: String,
    /// The settings in effect. See [`settings_text`].
    pub settings: String,
    /// SHA-256 of the instructions text, in lowercase hex.
    pub instructions_sha256: String,
}

impl ServerRun {
    pub fn new(build: Build, config: &AppConfig) -> ServerRun {
        ServerRun {
            version: build.version.to_string(),
            commit: build.commit.to_string(),
            settings: settings_text(
                &config.limits,
                &config.allowed_origins,
                config.allowed_hosts.extra(),
                config.admission.key_source(),
            ),
            instructions_sha256: sha256_hex(&config.instructions),
        }
    }
}

/// Stores `run` as a new server run that started at `started_at`, and
/// returns its ID. New agents and messages are stamped with that ID.
pub async fn record(db: &Db, run: &ServerRun, started_at: DateTime<Utc>) -> StoreResult<i64> {
    let record = NewServerRunRecord {
        started_at: clock::to_text(started_at),
        version: run.version.clone(),
        git_commit: run.commit.clone(),
        settings: run.settings.clone(),
        instructions_sha256: run.instructions_sha256.clone(),
    };
    db.write(move |w| w.insert_server_run(&record)).await
}

/// The settings as text that stays the same for the same settings: one
/// `name=value` line each, in a fixed order, with times in milliseconds and
/// origins and added host names sorted. When an access key is set, the file it came from is the
/// last line; the key itself is never recorded.
pub fn settings_text(
    limits: &Limits,
    allowed_origins: &[String],
    allowed_hosts: &[String],
    access_key_file: Option<&str>,
) -> String {
    // Listing every field, with no `..`, makes a new limit fail to compile
    // here until it is added to the text.
    let Limits {
        max_response_bytes,
        default_limit,
        max_limit,
        max_body_chars,
        max_body_bytes,
        max_channel_name_chars,
        max_purpose_chars,
        sends_per_minute,
        default_wait,
        max_wait,
        last_seen_interval,
        member_preview,
        purpose_preview_chars,
        web_session_lifetime,
    } = limits;
    let mut lines = vec![
        format!("max_response_bytes={max_response_bytes}"),
        format!("default_limit={default_limit}"),
        format!("max_limit={max_limit}"),
        format!("max_body_chars={max_body_chars}"),
        format!("max_body_bytes={max_body_bytes}"),
        format!("max_channel_name_chars={max_channel_name_chars}"),
        format!("max_purpose_chars={max_purpose_chars}"),
        format!("sends_per_minute={sends_per_minute}"),
        format!("default_wait_ms={}", default_wait.as_millis()),
        format!("max_wait_ms={}", max_wait.as_millis()),
        format!("last_seen_interval_ms={}", last_seen_interval.as_millis()),
        format!("member_preview={member_preview}"),
        format!("purpose_preview_chars={purpose_preview_chars}"),
        format!(
            "web_session_lifetime_ms={}",
            web_session_lifetime.as_millis()
        ),
    ];
    let mut origins = allowed_origins.to_vec();
    origins.sort();
    lines.extend(origins.iter().map(|o| format!("allowed_origin={o}")));
    let mut hosts = allowed_hosts.to_vec();
    hosts.sort();
    lines.extend(hosts.iter().map(|h| format!("allowed_host={h}")));
    lines.extend(access_key_file.map(|f| format!("access_key_file={f}")));
    lines.join("\n")
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admission::{Admission, SharedKey};
    use crate::hosts::AllowedHosts;

    fn config(limits: Limits, origins: &[&str], instructions: &str) -> AppConfig {
        AppConfig {
            admission: Admission::Open,
            limits,
            allowed_origins: origins.iter().map(|o| o.to_string()).collect(),
            allowed_hosts: AllowedHosts::new(&[]).expect("loopback hosts"),
            instructions: instructions.into(),
        }
    }

    const TEST_BUILD: Build = Build {
        version: "9.9.9",
        commit: "abc123",
    };

    #[test]
    fn a_run_records_the_build_settings_and_instructions() {
        let run = ServerRun::new(TEST_BUILD, &config(Limits::DEFAULT, &[], ""));
        assert_eq!(run.version, "9.9.9");
        assert_eq!(run.commit, "abc123");
        assert_eq!(
            run.settings,
            settings_text(&Limits::DEFAULT, &[], &[], None)
        );
        // SHA-256 of the empty string.
        assert_eq!(
            run.instructions_sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn default_settings_text_is_fixed() {
        assert_eq!(
            settings_text(
                &Limits::DEFAULT,
                &["http://localhost:6274".into()],
                &["host.docker.internal".into()],
                None
            ),
            "max_response_bytes=8192\n\
             default_limit=20\n\
             max_limit=100\n\
             max_body_chars=500\n\
             max_body_bytes=8192\n\
             max_channel_name_chars=64\n\
             max_purpose_chars=500\n\
             sends_per_minute=30\n\
             default_wait_ms=30000\n\
             max_wait_ms=50000\n\
             last_seen_interval_ms=60000\n\
             member_preview=3\n\
             purpose_preview_chars=120\n\
             web_session_lifetime_ms=43200000\n\
             allowed_origin=http://localhost:6274\n\
             allowed_host=host.docker.internal"
        );
    }

    #[test]
    fn origin_order_does_not_change_the_text() {
        let a = settings_text(
            &Limits::DEFAULT,
            &["http://b".into(), "http://a".into()],
            &[],
            None,
        );
        let b = settings_text(
            &Limits::DEFAULT,
            &["http://a".into(), "http://b".into()],
            &[],
            None,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn host_order_does_not_change_the_text() {
        let a = settings_text(
            &Limits::DEFAULT,
            &[],
            &["b.example".into(), "a.example".into()],
            None,
        );
        let b = settings_text(
            &Limits::DEFAULT,
            &[],
            &["a.example".into(), "b.example".into()],
            None,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn added_hosts_give_a_different_run() {
        let base = ServerRun::new(TEST_BUILD, &config(Limits::DEFAULT, &[], ""));
        let mut with_host = config(Limits::DEFAULT, &[], "");
        with_host.allowed_hosts =
            AllowedHosts::new(&["host.docker.internal".into()]).expect("valid host");
        let changed = ServerRun::new(TEST_BUILD, &with_host);
        assert_ne!(base.settings, changed.settings);
        assert!(
            changed
                .settings
                .ends_with("\nallowed_host=host.docker.internal")
        );
    }

    #[test]
    fn a_run_records_the_key_file_but_not_the_key() {
        let key = "0123456789abcdef0123456789abcdef";
        let mut keyed = config(Limits::DEFAULT, &[], "");
        keyed.admission = Admission::SharedKey(SharedKey::new(key, "/keys/kkt").expect("key"));
        let run = ServerRun::new(TEST_BUILD, &keyed);
        assert!(run.settings.ends_with("\naccess_key_file=/keys/kkt"));
        assert!(!run.settings.contains(key));
        let open = ServerRun::new(TEST_BUILD, &config(Limits::DEFAULT, &[], ""));
        assert!(!open.settings.contains("access_key"));
    }

    #[test]
    fn different_settings_or_instructions_give_a_different_run() {
        let base = ServerRun::new(TEST_BUILD, &config(Limits::DEFAULT, &[], "hello"));
        let other_limits = Limits {
            max_limit: 99,
            ..Limits::DEFAULT
        };
        let changed = ServerRun::new(TEST_BUILD, &config(other_limits, &[], "hello"));
        assert_ne!(base.settings, changed.settings);
        let changed = ServerRun::new(TEST_BUILD, &config(Limits::DEFAULT, &[], "hello!"));
        assert_eq!(base.settings, changed.settings);
        assert_ne!(base.instructions_sha256, changed.instructions_sha256);
    }

    #[test]
    fn this_build_names_a_commit_or_says_unknown() {
        let commit = Build::CURRENT.commit;
        let hash = commit.strip_suffix("-dirty").unwrap_or(commit);
        assert!(
            commit == "unknown"
                || (hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit())),
            "unexpected commit text {commit:?}"
        );
    }
}
