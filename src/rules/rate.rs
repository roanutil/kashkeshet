//! A per-agent cap on how fast messages can be sent.
//!
//! Each agent has a bucket that holds up to `per_minute` sends and refills at
//! `per_minute` sends per minute. State lives in memory and resets on restart.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bucket {
    available: f64,
    updated: DateTime<Utc>,
}

/// Takes one send from `bucket` at time `now`. Returns the new bucket and
/// whether the send is allowed.
fn take(bucket: Option<Bucket>, now: DateTime<Utc>, per_minute: u32) -> (Bucket, bool) {
    let capacity = f64::from(per_minute);
    let available = match bucket {
        None => capacity,
        Some(b) => {
            let elapsed = (now - b.updated).num_milliseconds().max(0) as f64 / 60_000.0;
            (b.available + elapsed * capacity).min(capacity)
        }
    };
    if available >= 1.0 {
        (
            Bucket {
                available: available - 1.0,
                updated: now,
            },
            true,
        )
    } else {
        (
            Bucket {
                available,
                updated: now,
            },
            false,
        )
    }
}

pub struct SendLimiter {
    per_minute: u32,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl SendLimiter {
    pub fn new(per_minute: u32) -> SendLimiter {
        SendLimiter {
            per_minute,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Records one send by `agent` at `now`. Returns false if the agent is over its limit.
    pub fn allow(&self, agent: &str, now: DateTime<Utc>) -> bool {
        // A poisoned lock means another thread panicked mid-update; the map is
        // still usable because each entry is replaced whole.
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        let (bucket, allowed) = take(buckets.get(agent).copied(), now, self.per_minute);
        buckets.insert(agent.to_string(), bucket);
        allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn allows_a_burst_then_refills() {
        let limiter = SendLimiter::new(3);
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert!(limiter.allow("a", t0));
        assert!(limiter.allow("a", t0));
        assert!(limiter.allow("a", t0));
        assert!(!limiter.allow("a", t0));
        // Another agent has its own bucket.
        assert!(limiter.allow("b", t0));
        // One third of a minute refills one send.
        assert!(limiter.allow("a", t0 + Duration::seconds(20)));
        assert!(!limiter.allow("a", t0 + Duration::seconds(20)));
    }
}
