//! The current time, injected so tests can control it.

use chrono::{DateTime, SecondsFormat, Utc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The real system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// The text form used for every stored time, e.g. `2026-10-23T14:02:11Z`.
/// Times in this form sort correctly as strings.
pub fn to_text(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parses a stored time. Returns `None` if the text is not a stored time.
pub fn from_text(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
pub mod testing {
    use super::Clock;
    use chrono::{DateTime, Duration, Utc};
    use std::sync::Mutex;

    /// A clock that only moves when told to.
    pub struct FixedClock(Mutex<DateTime<Utc>>);

    impl FixedClock {
        pub fn new(start: DateTime<Utc>) -> Self {
            Self(Mutex::new(start))
        }

        pub fn advance(&self, by: Duration) {
            let mut now = self.0.lock().expect("clock lock");
            *now += by;
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().expect("clock lock")
        }
    }
}
