//! The current time, injected so tests can control it.

use chrono::{DateTime, SecondsFormat, Utc};
use futures::future::BoxFuture;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;

    /// Finishes once this clock reaches `deadline`. A deadline already passed
    /// finishes at once.
    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'_, ()>;
}

/// The real system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'_, ()> {
        // A negative remaining time cannot be converted, so it becomes zero.
        let remaining = (deadline - Utc::now())
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        Box::pin(tokio::time::sleep(remaining))
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
    use futures::future::BoxFuture;
    use tokio::sync::watch;

    /// A clock that only moves when told to. Sleeps finish when `advance`
    /// moves it past their deadline.
    pub struct FixedClock(watch::Sender<DateTime<Utc>>);

    impl FixedClock {
        pub fn new(start: DateTime<Utc>) -> Self {
            Self(watch::Sender::new(start))
        }

        pub fn advance(&self, by: Duration) {
            self.0.send_modify(|now| *now += by);
        }

        /// How many sleeps are waiting on this clock. Tests use it to know a
        /// sleep has started before they move the clock.
        pub fn sleepers(&self) -> usize {
            self.0.receiver_count()
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.borrow()
        }

        fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'_, ()> {
            // Subscribe now, not on first poll, so `sleepers` counts this sleep
            // as soon as it is created.
            let mut now = self.0.subscribe();
            Box::pin(async move {
                // `wait_for` fails only if the sender is dropped, and the
                // sender lives as long as `self`, which this future borrows.
                now.wait_for(|t| *t >= deadline)
                    .await
                    .expect("clock outlives its sleeps");
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FixedClock;
    use super::*;
    use chrono::{Duration, TimeZone};
    use futures::FutureExt;

    fn start() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn fixed_clock_sleep_finishes_only_once_advanced_to_the_deadline() {
        let clock = FixedClock::new(start());
        let mut sleep = clock.sleep_until(start() + Duration::seconds(10));
        assert_eq!(clock.sleepers(), 1);
        assert!((&mut sleep).now_or_never().is_none());
        clock.advance(Duration::seconds(9));
        assert!((&mut sleep).now_or_never().is_none());
        clock.advance(Duration::seconds(1));
        assert!(sleep.now_or_never().is_some());
        assert_eq!(clock.sleepers(), 0);
    }

    #[tokio::test]
    async fn fixed_clock_sleep_to_a_past_deadline_finishes_at_once() {
        let clock = FixedClock::new(start());
        let sleep = clock.sleep_until(start() - Duration::seconds(1));
        assert!(sleep.now_or_never().is_some());
    }

    #[tokio::test]
    async fn system_clock_sleep_to_a_past_deadline_finishes_at_once() {
        let clock = SystemClock;
        let sleep = clock.sleep_until(Utc::now() - Duration::hours(1));
        tokio::time::timeout(std::time::Duration::from_secs(1), sleep)
            .await
            .expect("a past deadline should not wait");
    }
}
