//! Server limits. Every default is written out here, in one place.

use std::time::Duration;

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

    /// Clamps an agent's requested `limit` to `1..=max_limit`, using the default when absent.
    pub fn limit(&self, requested: Option<u32>) -> usize {
        match requested {
            Some(n) => (n as usize).clamp(1, self.max_limit),
            None => self.default_limit,
        }
    }
}
