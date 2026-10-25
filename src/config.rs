//! Server limits and fixed settings. Every default is written out here, in
//! one place, along with the checks a server's settings must pass at startup.

use std::net::SocketAddr;
use std::time::Duration;

// ---- Fixed settings ----
//
// Values agents never see and the command line does not change.

/// Read-only database connections kept open beside the one writer.
pub const DB_READERS: usize = 4;
/// How long a database connection waits for a lock before failing.
pub const DB_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// New-message notices kept for a slow subscriber before it starts missing them.
pub const EVENT_BUFFER: usize = 1024;
/// Messages shown on the web page's front page.
pub const WEB_RECENT_MESSAGES: usize = 50;
/// Messages shown on the web page for one channel.
pub const WEB_CHANNEL_PAGE_MESSAGES: usize = 200;
/// How often the web page's event stream sends a keep-alive comment.
pub const WEB_KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How many numbered names to try before giving up on a base agent name.
pub const MAX_NAME_ATTEMPTS: u32 = 10_000;
/// How many random channel IDs to try before giving up.
pub const MAX_CHANNEL_ID_ATTEMPTS: u32 = 100;
/// How often an MCP event stream sends a keep-alive ping.
pub const MCP_SSE_KEEP_ALIVE: Duration = Duration::from_secs(15);
/// The reconnect delay an MCP event stream suggests to its client.
pub const MCP_SSE_RETRY: Duration = Duration::from_secs(3);
/// Largest MCP request, in bytes. Requests over it get `413 Payload Too Large`.
pub const MCP_MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
/// `Host` names the server always accepts, on any port, for both `/mcp` and
/// the web page. `--allow-host` adds more. See `hosts`.
pub const ALWAYS_ALLOWED_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];
/// Shortest access key, in bytes. 32 random hex digits carry 128 bits.
pub const MIN_ACCESS_KEY_BYTES: usize = 32;
/// Smallest allowed `max_response_bytes`. A smaller cap could cut a
/// registration answer, and with it the agent's token.
pub const MIN_RESPONSE_BYTES: usize = 1024;

// ---- Limits ----

#[derive(Clone, Debug)]
pub struct Limits {
    /// Largest response, in bytes, before the server cuts it and says how to get the rest.
    pub max_response_bytes: usize,
    /// Items returned by list and read tools when the agent gives no `limit`.
    pub default_limit: usize,
    /// Largest `limit` an agent may ask for.
    pub max_limit: usize,
    /// Message bodies longer than this many characters are cut in listings.
    pub max_body_chars: usize,
    /// Largest message body, in bytes.
    pub max_body_bytes: usize,
    /// Longest channel name, in characters.
    pub max_channel_name_chars: usize,
    /// Longest channel purpose, in characters.
    pub max_purpose_chars: usize,
    /// Messages one agent may send per minute.
    pub sends_per_minute: u32,
    /// How long `wait_for_messages` waits when the agent gives no `timeout_s`.
    pub default_wait: Duration,
    /// Longest wait an agent may ask for.
    pub max_wait: Duration,
    /// How often an agent's `last_seen` time may be written.
    pub last_seen_interval: Duration,
    /// Member names shown with each channel in `list_channels`.
    pub member_preview: usize,
    /// Characters of a channel purpose shown in `list_channels`.
    pub purpose_preview_chars: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        max_response_bytes: 8 * 1024,
        default_limit: 20,
        max_limit: 100,
        max_body_chars: 500,
        max_body_bytes: 8 * 1024,
        max_channel_name_chars: 64,
        max_purpose_chars: 500,
        sends_per_minute: 30,
        default_wait: Duration::from_secs(30),
        // Common MCP clients give up on a tool call after about 60 seconds.
        max_wait: Duration::from_secs(50),
        last_seen_interval: Duration::from_secs(60),
        member_preview: 3,
        purpose_preview_chars: 120,
    };

    /// Checks that these limits make sense together. The server refuses to
    /// start with limits that fail.
    pub fn check(&self) -> Result<(), InvalidSetting> {
        let at_least_one = [
            ("default_limit", self.default_limit),
            ("max_body_chars", self.max_body_chars),
            ("max_body_bytes", self.max_body_bytes),
            ("max_channel_name_chars", self.max_channel_name_chars),
            ("max_purpose_chars", self.max_purpose_chars),
            ("sends_per_minute", self.sends_per_minute as usize),
        ];
        if let Some((name, _)) = at_least_one.iter().find(|(_, value)| *value == 0) {
            return Err(InvalidSetting::new(name, "must be at least 1"));
        }
        if self.max_response_bytes < MIN_RESPONSE_BYTES {
            return Err(InvalidSetting::new(
                "max_response_bytes",
                format!("must be at least {MIN_RESPONSE_BYTES}"),
            ));
        }
        if self.max_limit < self.default_limit {
            return Err(InvalidSetting::new(
                "max_limit",
                "must be at least default_limit",
            ));
        }
        if self.max_wait < self.default_wait {
            return Err(InvalidSetting::new(
                "max_wait",
                "must be at least default_wait",
            ));
        }
        Ok(())
    }

    /// Clamps an agent's requested `limit` to `1..=max_limit`, using the default when absent.
    pub fn limit(&self, requested: Option<u32>) -> usize {
        match requested {
            Some(n) => (n as usize).clamp(1, self.max_limit),
            None => self.default_limit,
        }
    }
}

/// A setting the server refuses to start with.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{name} {rule}")]
pub struct InvalidSetting {
    pub name: &'static str,
    pub rule: String,
}

impl InvalidSetting {
    pub(crate) fn new(name: &'static str, rule: impl Into<String>) -> InvalidSetting {
        InvalidSetting {
            name,
            rule: rule.into(),
        }
    }
}

/// Checks the listen address. Only loopback addresses are allowed: the server
/// has no authentication beyond agent tokens and is meant for one machine.
pub fn check_bind(addr: SocketAddr) -> Result<(), InvalidSetting> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(InvalidSetting::new(
            "bind",
            format!("must be a loopback address (such as 127.0.0.1), not {addr}"),
        ))
    }
}

/// Checks an access key, as read from `--access-key-file` without
/// surrounding whitespace. It must be at least [`MIN_ACCESS_KEY_BYTES`] long
/// and only printable ASCII with no spaces, so it can be sent in an HTTP
/// header as written.
pub fn check_access_key(key: &str) -> Result<(), InvalidSetting> {
    if key.is_empty() {
        return Err(InvalidSetting::new("access_key_file", "must not be empty"));
    }
    if !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(InvalidSetting::new(
            "access_key_file",
            "must hold only printable ASCII with no spaces",
        ));
    }
    if key.len() < MIN_ACCESS_KEY_BYTES {
        return Err(InvalidSetting::new(
            "access_key_file",
            format!("must hold a key of at least {MIN_ACCESS_KEY_BYTES} characters"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_pass() {
        assert_eq!(Limits::DEFAULT.check(), Ok(()));
    }

    #[test]
    fn zero_is_refused_for_every_count() {
        type SetZero = fn(&mut Limits);
        let cases: [(&str, SetZero); 6] = [
            ("default_limit", |l| l.default_limit = 0),
            ("max_body_chars", |l| l.max_body_chars = 0),
            ("max_body_bytes", |l| l.max_body_bytes = 0),
            ("max_channel_name_chars", |l| l.max_channel_name_chars = 0),
            ("max_purpose_chars", |l| l.max_purpose_chars = 0),
            ("sends_per_minute", |l| l.sends_per_minute = 0),
        ];
        for (name, set_zero) in cases {
            let mut limits = Limits::DEFAULT;
            set_zero(&mut limits);
            let err = limits.check().expect_err(name);
            assert_eq!(err.name, name);
        }
    }

    #[test]
    fn response_cap_has_a_floor() {
        let limits = Limits {
            max_response_bytes: MIN_RESPONSE_BYTES - 1,
            ..Limits::DEFAULT
        };
        assert_eq!(
            limits.check().map_err(|e| e.name),
            Err("max_response_bytes")
        );
        let limits = Limits {
            max_response_bytes: MIN_RESPONSE_BYTES,
            ..Limits::DEFAULT
        };
        assert_eq!(limits.check(), Ok(()));
    }

    #[test]
    fn maximums_may_not_be_below_defaults() {
        let limits = Limits {
            default_limit: 10,
            max_limit: 9,
            ..Limits::DEFAULT
        };
        assert_eq!(limits.check().map_err(|e| e.name), Err("max_limit"));
        let limits = Limits {
            default_limit: 10,
            max_limit: 10,
            ..Limits::DEFAULT
        };
        assert_eq!(limits.check(), Ok(()));

        let limits = Limits {
            default_wait: Duration::from_secs(10),
            max_wait: Duration::from_secs(9),
            ..Limits::DEFAULT
        };
        assert_eq!(limits.check().map_err(|e| e.name), Err("max_wait"));
    }

    #[test]
    fn only_loopback_addresses_may_be_bound() {
        assert_eq!(check_bind("127.0.0.1:7420".parse().expect("addr")), Ok(()));
        assert_eq!(check_bind("[::1]:7420".parse().expect("addr")), Ok(()));
        let err = check_bind("0.0.0.0:7420".parse().expect("addr")).expect_err("refused");
        assert_eq!(err.name, "bind");
    }

    #[test]
    fn access_keys_must_be_long_printable_and_unspaced() {
        let long = "k".repeat(MIN_ACCESS_KEY_BYTES);
        assert_eq!(check_access_key(&long), Ok(()));
        let short = "k".repeat(MIN_ACCESS_KEY_BYTES - 1);
        let spaced = format!("{long} {long}");
        let accented = format!("{long}é");
        for bad in ["", short.as_str(), spaced.as_str(), accented.as_str()] {
            let err = check_access_key(bad).expect_err(bad);
            assert_eq!(err.name, "access_key_file");
        }
    }
}
