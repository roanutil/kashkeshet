//! Server limits and fixed settings. Every default is written out here, in
//! one place, along with the checks a server's settings must pass at startup.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

// ---- Fixed settings ----
//
// Values agents never see and the command line does not change.

/// Prefix of an agent authentication token.
pub const TOKEN_PREFIX: &str = "act_";
/// Prefix of an identity recovery code.
pub const RECOVERY_PREFIX: &str = "arc_";
/// Random characters per agent token or recovery code (about 134 bits).
pub const AGENT_SECRET_CHARS: usize = 26;
/// Read-only database connections kept open beside the one writer.
pub const DB_READERS: usize = 4;
/// How long a database connection waits for a lock before failing.
pub const DB_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// New-message notices kept for a slow subscriber before it starts missing them.
pub const EVENT_BUFFER: usize = 1024;
/// The front page counts each channel's messages in this last stretch of
/// time, to show which channels are busy.
pub const WEB_ACTIVITY_WINDOW: Duration = Duration::from_secs(5 * 60);
/// Those messages are counted in stretches this long, so the page gets a few
/// numbers per channel rather than a time for every message, and a count
/// leaves the window at most this late.
pub const WEB_ACTIVITY_BUCKET: Duration = Duration::from_secs(10);
/// Largest `web_page_messages`. More would make a page slow to load and
/// heavy to keep live.
pub const MAX_WEB_PAGE_MESSAGES: usize = 5000;
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
/// Characters in a web session ID, from the 36-character alphabet: about
/// 165 bits.
pub const WEB_SESSION_ID_CHARS: usize = 32;
/// The cookie that carries a web session ID.
pub const WEB_SESSION_COOKIE: &str = "kkt_session";
/// Shortest access key, in bytes. 32 random hex digits carry 128 bits.
pub const MIN_ACCESS_KEY_BYTES: usize = 32;
/// Characters in a key made by `kkt key create`, from the 36-character
/// alphabet: about 258 bits.
pub const NEW_ACCESS_KEY_CHARS: usize = 50;
const _: () = assert!(NEW_ACCESS_KEY_CHARS >= MIN_ACCESS_KEY_BYTES);
/// Unix permissions of a key file `kkt key create` writes: only the owner
/// may read or write it.
pub const ACCESS_KEY_FILE_MODE: u32 = 0o600;
/// Unix permissions of a backup's folder, which holds a copy of the access
/// key: only the owner may list or enter it.
pub const BACKUP_FOLDER_MODE: u32 = 0o700;
/// The folder, beside the database file, where the server keeps the backups
/// of the database it makes before upgrading it.
pub const UPGRADE_BACKUP_FOLDER: &str = "upgrade-backups";
/// The Compose project name for kkt in Docker, so the containers and volumes
/// are named the same whatever the folder holding the Compose files is called.
pub const DOCKER_PROJECT: &str = "kkt";
/// The folder, in the per-user data directory, that `kkt setup` keeps
/// the Docker install's files in.
pub const DOCKER_FOLDER: &str = "docker";
/// Where `kkt setup` saves the Docker server's access key, in the config
/// directory.
pub const DOCKER_KEY_FILE_NAME: &str = "docker-access-key";
/// The machine name `kkt setup` asks for on the tailnet.
pub const DEFAULT_TS_HOSTNAME: &str = "kkt";
/// How long `kkt setup` waits for a server it just set up to answer before
/// its connection check gives up. A new Tailscale node gets its HTTPS
/// certificate on the first request, which can take a while.
pub const SETUP_SERVER_WAIT: Duration = Duration::from_secs(2 * 60);
/// How often `kkt setup` asks while it waits.
pub const SETUP_SERVER_POLL: Duration = Duration::from_secs(2);
/// The user ID the kkt image runs as. A host folder it writes backups to
/// must belong to it.
pub const DOCKER_IMAGE_USER_ID: u32 = 10001;
/// The folder inside the managed Docker folder that is mounted at
/// [`DOCKER_BACKUPS_MOUNT`] in the container.
pub const DOCKER_BACKUPS_DIR: &str = "backups";
/// Where the Compose file mounts the backups folder in the container.
pub const DOCKER_BACKUPS_MOUNT: &str = "/backups";
/// The environment variables Compose reads, which would take the place of
/// the managed folder's settings file if they were set where kkt runs, so
/// every Docker command runs without them.
pub const DOCKER_MANAGED_VARIABLES: &[&str] = &[
    "COMPOSE_FILE",
    "COMPOSE_PATH_SEPARATOR",
    "COMPOSE_PROFILES",
    "COMPOSE_PROJECT_NAME",
    "COMPOSE_ENV_FILES",
    "KKT_VERSION",
    "KKT_PORT",
    "TS_HOSTNAME",
    "TS_TAILNET",
    "TS_AUTHKEY",
];
/// How long to wait for someone to log the Tailscale container in.
pub const DOCKER_LOGIN_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long to wait for the kkt container to start and make its access key.
pub const DOCKER_START_WAIT: Duration = Duration::from_secs(2 * 60);
/// How many checks to wait for a login link from a Tailscale container that
/// needs a login before restarting it. A node whose key expired, or that was
/// logged out, is not asking for a login, so it shows no link until it is
/// restarted; a new one shows its link within a few seconds.
pub const DOCKER_LINK_WAIT_POLLS: u32 = 5;
/// How often to ask while waiting for the Tailscale login or for kkt.
pub const DOCKER_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// What `TS_TAILNET` is set to until the Tailscale container has logged in
/// and the real name is known. The Compose file needs some value to start
/// the Tailscale container alone; kkt does not run with it.
pub const DOCKER_PENDING_TAILNET: &str = "pending.invalid";
/// Smallest allowed `max_response_bytes`. A smaller cap could cut a
/// registration answer, and with it the agent's token.
pub const MIN_RESPONSE_BYTES: usize = 1024;

/// Characters of a pinned note shown in channel previews.
pub const NOTE_PREVIEW_CHARS: usize = 120;

// ---- Server defaults and where files live ----
//
// What a server uses when neither a flag nor the config file says otherwise.

/// Address to listen on.
pub const DEFAULT_BIND: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7420));
/// Whether `bind` may be a non-loopback address.
pub const DEFAULT_ALLOW_ANY_BIND: bool = false;
/// Database file name in the per-user data directory, for `kkt serve`.
pub const DB_FILE_NAME: &str = "kashkeshet.db";
/// The per-user data directory on macOS, under the home directory.
pub const MACOS_DATA_DIR: &str = "Library/Application Support/kashkeshet";
/// The per-user data directory elsewhere, under `$XDG_DATA_HOME`.
pub const XDG_DATA_DIR: &str = "kashkeshet";
/// `$XDG_DATA_HOME` when it is not set, under the home directory.
pub const XDG_DATA_HOME_DEFAULT: &str = ".local/share";
/// The config directory, under `$XDG_CONFIG_HOME`, on every platform.
pub const CONFIG_DIR: &str = "kkt";
/// `$XDG_CONFIG_HOME` when it is not set, under the home directory.
pub const XDG_CONFIG_HOME_DEFAULT: &str = ".config";
/// The config file `kkt serve` reads, in the config directory, if it exists.
pub const CONFIG_FILE_NAME: &str = "config.toml";
/// Where `kkt key create` writes a key, in the config directory.
pub const ACCESS_KEY_FILE_NAME: &str = "access-key";
/// The setup record `kkt setup` writes, in the config directory.
pub const SETUP_FILE_NAME: &str = "setup.toml";
/// The zero-based mode option Enter accepts during interactive setup (Docker).
pub const DEFAULT_SETUP_MODE_INDEX: usize = 0;
/// Whether Enter allows agents in local containers during interactive setup.
pub const DEFAULT_SETUP_CONTAINERS: bool = true;
/// Where `kkt setup` keeps a client-only machine's copy of its server's
/// key, in the config directory.
pub const CLIENT_KEY_FILE_NAME: &str = "client-access-key";
/// The host Tailscale Serve listener used by service-mode setup.
pub const TAILSCALE_HTTPS_PORT: u16 = 443;
/// Full backups made before an explicit service-mode upgrade, in the
/// user's kkt config directory.
pub const SERVICE_BACKUP_FOLDER: &str = "backups";

// Client setup and diagnostics.
pub const CODEX_CONFIG: &str = ".codex/config.toml";
pub const CLAUDE_CONFIG: &str = ".claude.json";
pub const AGY_CONFIG: &str = ".gemini/config/mcp_config.json";
pub const AGY_LEGACY_CONFIG: &str = ".gemini/antigravity/mcp_config.json";
pub const OPENCODE_CONFIG: &str = ".config/opencode/opencode.json";
/// Muse Code's user settings, under the config home (`$XDG_CONFIG_HOME`,
/// or `~/.config`), so `~/.config/muse/settings.json` by default.
pub const MUSE_CONFIG: &str = "muse/settings.json";
/// The programs `kkt setup` looks for on PATH to tell which clients are
/// installed.
pub const CLAUDE_PROGRAM: &str = "claude";
pub const CODEX_PROGRAM: &str = "codex";
pub const OPENCODE_PROGRAM: &str = "opencode";
pub const MUSE_PROGRAM: &str = "muse";
pub const DOCTOR_TIMEOUT: Duration = Duration::from_secs(5);
pub const DOCTOR_MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const DOCTOR_PROTOCOL_VERSION: &str = "2026-07-28";

// ---- The web preview ----
//
// The page with simulated data (`web::Preview`, run by the `web_preview`
// example), for working on the page without a server or agents.

/// Address the preview listens on: next to the server's default port.
pub const PREVIEW_BIND: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7421));
/// Seed for the simulated data when none is given.
pub const PREVIEW_SEED: u64 = 1;
/// Agents in the simulated world at the start.
pub const PREVIEW_AGENTS: usize = 8;
/// Channels at the start. One of them stays empty until the feed reaches it.
pub const PREVIEW_CHANNELS: usize = 7;
/// Messages at the start, spread over the last few hours. Half go to the
/// first channel, so it has more than one page.
pub const PREVIEW_MESSAGES: usize = 600;
/// How far back the starting messages go.
pub const PREVIEW_HISTORY: Duration = Duration::from_secs(6 * 60 * 60);
/// One step in this many is a burst of messages.
pub const PREVIEW_BURST_ONE_IN: usize = 12;
/// Most messages in a burst.
pub const PREVIEW_BURST_MAX: usize = 15;
/// One step in this many is a new agent registering.
pub const PREVIEW_NEW_AGENT_ONE_IN: usize = 25;
/// One step in this many is a new channel.
pub const PREVIEW_NEW_CHANNEL_ONE_IN: usize = 40;
/// One message in this many is very long.
pub const PREVIEW_LONG_MESSAGE_ONE_IN: usize = 10;

/// What the preview's operator can change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewSettings {
    /// Steps of the simulated feed per minute. Most steps are one message.
    pub steps_per_minute: u32,
    /// The most messages one page shows, as the server's `web_page_messages`.
    pub page_messages: usize,
}

impl PreviewSettings {
    pub const DEFAULT: PreviewSettings = PreviewSettings {
        steps_per_minute: 30,
        page_messages: Limits::DEFAULT.web_page_messages,
    };

    /// Checks the settings. The preview refuses to start with ones that fail.
    pub fn check(&self) -> Result<(), InvalidSetting> {
        if self.steps_per_minute == 0 || self.steps_per_minute > 60_000 {
            return Err(InvalidSetting::new(
                "rate",
                "must be from 1 to 60000 steps per minute",
            ));
        }
        if self.page_messages == 0 || self.page_messages > MAX_WEB_PAGE_MESSAGES {
            return Err(InvalidSetting::new(
                "page_messages",
                format!("must be from 1 to {MAX_WEB_PAGE_MESSAGES}"),
            ));
        }
        Ok(())
    }
}

// ---- Running as a service ----
//
// What `kkt service` writes and runs. See `service`.

/// The launchd job label on macOS. The plist is `<label>.plist`.
pub const LAUNCHD_LABEL: &str = "com.sam-mfb.kashkeshet";
/// Where launchd user agents live, under the home directory.
pub const LAUNCH_AGENTS_DIR: &str = "Library/LaunchAgents";
/// Where the service's log goes on macOS, under the home directory. Linux
/// services log to the journal.
pub const MACOS_LOG_DIR: &str = "Library/Logs/kashkeshet";
/// The service's log file on macOS, in [`MACOS_LOG_DIR`]. The server writes
/// it itself (`--log-file`), within [`LogPolicy::DEFAULT`].
pub const MACOS_LOG_FILE_NAME: &str = "kashkeshet.log";
/// Where launchd sends the service's stdout and stderr on macOS, in
/// [`MACOS_LOG_DIR`]. Only output from before logging starts, and crashes,
/// land here.
pub const MACOS_CRASH_LOG_FILE_NAME: &str = "kashkeshet.crash.log";
/// The systemd user unit on Linux.
pub const SYSTEMD_UNIT: &str = "kashkeshet.service";
/// Where systemd user units live, under `$XDG_CONFIG_HOME`.
pub const SYSTEMD_USER_DIR: &str = "systemd/user";
/// How long the service manager waits before starting the server again
/// after it failed.
pub const SERVICE_RESTART_DELAY: Duration = Duration::from_secs(5);
/// How long the service manager waits for the server to stop before it
/// kills it.
pub const SERVICE_STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// Log lines `kkt service logs` shows when not told how many.
pub const DEFAULT_LOG_LINES: usize = 100;

// ---- Logging ----

/// How much is logged: the named level and everything more severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// Every level, least verbose first, as named in flags and the config file.
    pub const ALL: [(LogLevel, &'static str); 5] = [
        (LogLevel::Error, "error"),
        (LogLevel::Warn, "warn"),
        (LogLevel::Info, "info"),
        (LogLevel::Debug, "debug"),
        (LogLevel::Trace, "trace"),
    ];

    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }

    /// The level named `text`, or an error naming `setting` and the choices.
    pub fn parse(setting: &'static str, text: &str) -> Result<LogLevel, InvalidSetting> {
        LogLevel::ALL
            .iter()
            .find(|(_, name)| *name == text)
            .map(|(level, _)| *level)
            .ok_or_else(|| {
                InvalidSetting::new(
                    setting,
                    format!("must be one of error, warn, info, debug, trace, not {text:?}"),
                )
            })
    }
}

/// The level logged for kkt's own code (`log_level`).
pub const DEFAULT_LOG_LEVEL: LogLevel = LogLevel::Info;
/// The level logged for the libraries kkt uses (`dependency_log_level`).
/// Above info, because the MCP library logs several lines for every request.
pub const DEFAULT_DEPENDENCY_LOG_LEVEL: LogLevel = LogLevel::Warn;

/// Checks `dependency_log_level`. At debug and trace the MCP library logs
/// whole requests and replies, tokens and recovery codes included, and logs
/// must never hold those; so those levels are refused.
pub fn check_dependency_log_level(level: LogLevel) -> Result<(), InvalidSetting> {
    match level {
        LogLevel::Error | LogLevel::Warn | LogLevel::Info => Ok(()),
        LogLevel::Debug | LogLevel::Trace => Err(InvalidSetting::new(
            "dependency_log_level",
            format!(
                "must be error, warn or info, not {}: at debug and trace the MCP library \
                 logs request contents, including tokens",
                level.name()
            ),
        )),
    }
}
/// The targets of kkt's own code: the library and the `kkt` binary.
pub const LOG_OWN_TARGETS: &[&str] = &["kashkeshet", "kkt"];
/// How often a log file the server writes itself looks for rolled files past
/// their age, besides when it opens and when it rolls over. It looks on the
/// first write after this much time, so a server that logs nothing deletes
/// nothing until it next logs.
pub const LOG_PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Smallest allowed [`LogPolicy::max_bytes`].
pub const MIN_LOG_FILE_BYTES: u64 = 64 * 1024;

/// How a log file the server writes itself (`--log-file`) is kept bounded.
/// When the file would grow past `max_bytes`, it is renamed `<file>.1`, the
/// older ones move up to `<file>.<rotated_files>`, and the oldest is
/// deleted. Rolled files older than `max_age` are deleted too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogPolicy {
    pub max_bytes: u64,
    pub rotated_files: usize,
    pub max_age: Duration,
}

impl LogPolicy {
    /// At most about 60 MB: the current file and five rolled ones of 10 MB,
    /// none rolled more than 30 days ago.
    pub const DEFAULT: LogPolicy = LogPolicy {
        max_bytes: 10 * 1024 * 1024,
        rotated_files: 5,
        max_age: Duration::from_secs(30 * 24 * 60 * 60),
    };

    /// Checks that the policy bounds the log. The server refuses to start
    /// with one that fails.
    pub fn check(&self) -> Result<(), InvalidSetting> {
        if self.max_bytes < MIN_LOG_FILE_BYTES {
            return Err(InvalidSetting::new(
                "log_max_bytes",
                format!("must be at least {MIN_LOG_FILE_BYTES}"),
            ));
        }
        if self.rotated_files == 0 {
            return Err(InvalidSetting::new(
                "log_rotated_files",
                "must be at least 1",
            ));
        }
        if self.max_age < Duration::from_secs(60 * 60) {
            return Err(InvalidSetting::new(
                "log_max_age",
                "must be at least an hour",
            ));
        }
        Ok(())
    }
}

// ---- Limits ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest response, in bytes, before the server cuts it and says how to get the rest.
    pub max_response_bytes: usize,
    /// Items returned by list and read tools when the agent gives no `limit`.
    pub default_limit: usize,
    /// Largest `limit` an agent may ask for.
    pub max_limit: usize,
    /// Largest message body, in bytes.
    pub max_body_bytes: usize,
    /// Longest channel name, in characters.
    pub max_channel_name_chars: usize,
    /// Longest channel purpose, in characters.
    pub max_purpose_chars: usize,
    /// Longest agent description, in characters.
    pub max_description_chars: usize,
    /// Longest pinned channel note, in characters.
    pub max_note_chars: usize,
    /// Longest operator instruction text, in characters.
    pub max_operator_instructions_chars: usize,
    /// Longest handoff note an agent keeps for itself, in characters.
    pub max_handoff_note_chars: usize,
    /// Messages one agent may send per minute.
    pub sends_per_minute: u32,
    /// Agents the whole server may register per minute.
    pub registrations_per_minute: u32,
    /// Most agents that may exist at once, not counting revoked ones.
    pub max_agents: usize,
    /// How long `wait_for_messages` waits when the agent gives no `timeout_s`.
    pub default_wait: Duration,
    /// Longest wait an agent may ask for.
    pub max_wait: Duration,
    /// How long a normally completed wait remains visible as listening.
    pub waiting_grace: Duration,
    /// Member names shown with each channel in `list_channels`.
    pub member_preview: usize,
    /// Characters of a channel purpose shown in `list_channels`.
    pub purpose_preview_chars: usize,
    /// How long a web page sign-in lasts, when an access key is set.
    pub web_session_lifetime: Duration,
    /// The most messages one web page shows at first: a channel's page, and
    /// the live view, which drops the oldest as new ones arrive. A channel
    /// page's "Show previous" button loads this many earlier ones at a time.
    pub web_page_messages: usize,
    /// How many of the backups the server makes before upgrading the
    /// database it keeps; older ones are removed.
    pub upgrade_backups_kept: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        max_response_bytes: 8 * 1024,
        default_limit: 20,
        max_limit: 100,
        max_body_bytes: 8 * 1024,
        max_channel_name_chars: 64,
        max_purpose_chars: 500,
        max_description_chars: 300,
        max_note_chars: 10_000,
        max_operator_instructions_chars: 4000,
        max_handoff_note_chars: 20_000,
        sends_per_minute: 30,
        registrations_per_minute: 20,
        max_agents: 1000,
        default_wait: Duration::from_secs(30),
        // Common MCP clients give up on a tool call after about 60 seconds.
        max_wait: Duration::from_secs(50),
        waiting_grace: Duration::from_secs(3),
        member_preview: 3,
        purpose_preview_chars: 120,
        web_session_lifetime: Duration::from_secs(12 * 60 * 60),
        web_page_messages: 50,
        upgrade_backups_kept: 5,
    };

    /// Checks that these limits make sense together. The server refuses to
    /// start with limits that fail.
    pub fn check(&self) -> Result<(), InvalidSetting> {
        let at_least_one = [
            ("default_limit", self.default_limit),
            ("max_body_bytes", self.max_body_bytes),
            ("max_channel_name_chars", self.max_channel_name_chars),
            ("max_purpose_chars", self.max_purpose_chars),
            ("max_description_chars", self.max_description_chars),
            ("max_note_chars", self.max_note_chars),
            (
                "max_operator_instructions_chars",
                self.max_operator_instructions_chars,
            ),
            ("max_handoff_note_chars", self.max_handoff_note_chars),
            ("sends_per_minute", self.sends_per_minute as usize),
            (
                "registrations_per_minute",
                self.registrations_per_minute as usize,
            ),
            ("max_agents", self.max_agents),
            ("web_page_messages", self.web_page_messages),
            ("upgrade_backups_kept", self.upgrade_backups_kept),
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
        if self.web_page_messages > MAX_WEB_PAGE_MESSAGES {
            return Err(InvalidSetting::new(
                "web_page_messages",
                format!("must be at most {MAX_WEB_PAGE_MESSAGES}"),
            ));
        }
        if self.web_session_lifetime < Duration::from_secs(1) {
            return Err(InvalidSetting::new(
                "web_session_lifetime",
                "must be at least 1 second",
            ));
        }
        if self.waiting_grace > self.max_wait {
            return Err(InvalidSetting::new(
                "waiting_grace",
                "must not exceed max_wait",
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

/// Checks the listen address. Only loopback addresses are allowed unless
/// `allow_any_bind` is set: the server is meant for one machine, and a
/// container needs to listen on its own network interface. `has_key` says
/// whether an access key guards the server; any other address needs one.
pub fn check_bind(
    addr: SocketAddr,
    allow_any_bind: bool,
    has_key: bool,
) -> Result<(), InvalidSetting> {
    if addr.ip().is_loopback() {
        return Ok(());
    }
    if !allow_any_bind {
        return Err(InvalidSetting::new(
            "bind",
            format!(
                "must be a loopback address (such as 127.0.0.1), not {addr}, \
                 unless allow_any_bind is set"
            ),
        ));
    }
    if !has_key {
        return Err(InvalidSetting::new(
            "allow_any_bind",
            "needs an access key: it cannot be used with no_access_key",
        ));
    }
    Ok(())
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
    fn web_pages_have_a_ceiling() {
        let limits = Limits {
            web_page_messages: MAX_WEB_PAGE_MESSAGES + 1,
            ..Limits::DEFAULT
        };
        assert_eq!(limits.check().map_err(|e| e.name), Err("web_page_messages"));
        let limits = Limits {
            web_page_messages: MAX_WEB_PAGE_MESSAGES,
            ..Limits::DEFAULT
        };
        assert_eq!(limits.check(), Ok(()));
    }

    #[test]
    fn preview_rates_must_be_sensible() {
        assert_eq!(PreviewSettings::DEFAULT.check(), Ok(()));
        for bad in [0, 60_001] {
            let settings = PreviewSettings {
                steps_per_minute: bad,
                ..PreviewSettings::DEFAULT
            };
            assert_eq!(settings.check().map_err(|e| e.name), Err("rate"));
        }
        for bad in [0, MAX_WEB_PAGE_MESSAGES + 1] {
            let settings = PreviewSettings {
                page_messages: bad,
                ..PreviewSettings::DEFAULT
            };
            assert_eq!(settings.check().map_err(|e| e.name), Err("page_messages"));
        }
    }

    #[test]
    fn default_limits_pass() {
        assert_eq!(Limits::DEFAULT.check(), Ok(()));
    }

    #[test]
    fn zero_is_refused_for_every_count() {
        type SetZero = fn(&mut Limits);
        let cases: [(&str, SetZero); 12] = [
            ("default_limit", |l| l.default_limit = 0),
            ("max_body_bytes", |l| l.max_body_bytes = 0),
            ("max_channel_name_chars", |l| l.max_channel_name_chars = 0),
            ("max_purpose_chars", |l| l.max_purpose_chars = 0),
            ("max_description_chars", |l| l.max_description_chars = 0),
            ("max_note_chars", |l| l.max_note_chars = 0),
            ("max_operator_instructions_chars", |l| {
                l.max_operator_instructions_chars = 0
            }),
            ("max_handoff_note_chars", |l| l.max_handoff_note_chars = 0),
            ("sends_per_minute", |l| l.sends_per_minute = 0),
            ("registrations_per_minute", |l| {
                l.registrations_per_minute = 0
            }),
            ("max_agents", |l| l.max_agents = 0),
            ("web_page_messages", |l| l.web_page_messages = 0),
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
    fn web_sessions_last_at_least_a_second() {
        let limits = Limits {
            web_session_lifetime: Duration::from_millis(999),
            ..Limits::DEFAULT
        };
        assert_eq!(
            limits.check().map_err(|e| e.name),
            Err("web_session_lifetime")
        );
    }

    #[test]
    fn only_loopback_addresses_may_be_bound() {
        let local: SocketAddr = "127.0.0.1:7420".parse().expect("addr");
        let local6: SocketAddr = "[::1]:7420".parse().expect("addr");
        let any: SocketAddr = "0.0.0.0:7420".parse().expect("addr");
        assert_eq!(check_bind(local, false, true), Ok(()));
        assert_eq!(check_bind(local6, false, false), Ok(()));
        let err = check_bind(any, false, true).expect_err("refused");
        assert_eq!(err.name, "bind");
    }

    #[test]
    fn any_address_may_be_bound_only_when_allowed_and_keyed() {
        let any: SocketAddr = "0.0.0.0:7420".parse().expect("addr");
        assert_eq!(check_bind(any, true, true), Ok(()));
        let err = check_bind(any, true, false).expect_err("refused");
        assert_eq!(err.name, "allow_any_bind");
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

    #[test]
    fn the_default_log_policy_passes_and_bad_ones_are_named() {
        assert_eq!(LogPolicy::DEFAULT.check(), Ok(()));
        let cases = [
            (
                LogPolicy {
                    max_bytes: MIN_LOG_FILE_BYTES - 1,
                    ..LogPolicy::DEFAULT
                },
                "log_max_bytes",
            ),
            (
                LogPolicy {
                    rotated_files: 0,
                    ..LogPolicy::DEFAULT
                },
                "log_rotated_files",
            ),
            (
                LogPolicy {
                    max_age: Duration::from_secs(59 * 60),
                    ..LogPolicy::DEFAULT
                },
                "log_max_age",
            ),
        ];
        for (policy, name) in cases {
            assert_eq!(policy.check().map_err(|e| e.name), Err(name));
        }
    }

    #[test]
    fn log_levels_parse_by_name_and_refuse_others() {
        for (level, name) in LogLevel::ALL {
            assert_eq!(LogLevel::parse("log_level", name), Ok(level));
            assert_eq!(level.name(), name);
        }
        let err = LogLevel::parse("dependency_log_level", "verbose").expect_err("unknown");
        assert_eq!(err.name, "dependency_log_level");
        assert!(LogLevel::parse("log_level", "INFO").is_err());
    }

    #[test]
    fn dependency_levels_that_would_log_secrets_are_refused() {
        for level in [LogLevel::Error, LogLevel::Warn, LogLevel::Info] {
            assert_eq!(check_dependency_log_level(level), Ok(()));
        }
        for level in [LogLevel::Debug, LogLevel::Trace] {
            let err = check_dependency_log_level(level).expect_err("refused");
            assert_eq!(err.name, "dependency_log_level");
        }
    }
}
