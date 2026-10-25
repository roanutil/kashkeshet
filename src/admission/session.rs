//! Sessions: who has signed in to the web page, and until when. Held in
//! memory, so a restart signs everyone out. A session says nothing about how
//! its holder signed in; today that is the shared key.

use super::Principal;
use crate::clock::Clock;
use crate::config::WEB_SESSION_ID_CHARS;
use crate::names::{self, Entropy};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The sessions in effect. Only a hash of each session ID is kept.
pub struct Sessions {
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    lifetime: Duration,
    live: Mutex<HashMap<[u8; 32], Session>>,
}

struct Session {
    principal: Principal,
    started: DateTime<Utc>,
}

/// Whether a presented session ID is good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    Live(Principal),
    /// Never issued, signed out, or expired.
    Unknown,
}

impl Sessions {
    /// Sessions that last `lifetime` from sign-in, timed by `clock`, with IDs
    /// drawn from `entropy`.
    pub fn new(clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, lifetime: Duration) -> Sessions {
        Sessions {
            clock,
            entropy,
            lifetime,
            live: Mutex::new(HashMap::new()),
        }
    }

    /// How long a session lasts.
    pub fn lifetime(&self) -> Duration {
        self.lifetime
    }

    /// Starts a session for `principal` and returns its ID, which only the
    /// caller ever sees. Expired sessions are dropped first.
    pub fn start(&self, principal: Principal) -> String {
        let id = names::random_chars(self.entropy.as_ref(), WEB_SESSION_ID_CHARS);
        let now = self.clock.now();
        let mut live = self.live();
        live.retain(|_, s| self.within_lifetime(s, now));
        live.insert(
            hash(&id),
            Session {
                principal,
                started: now,
            },
        );
        id
    }

    /// Looks up the session with ID `id`. An expired session is dropped.
    pub fn lookup(&self, id: &str) -> Lookup {
        let key = hash(id);
        let now = self.clock.now();
        let mut live = self.live();
        match live.get(&key) {
            Some(s) if self.within_lifetime(s, now) => Lookup::Live(s.principal),
            Some(_) => {
                live.remove(&key);
                Lookup::Unknown
            }
            None => Lookup::Unknown,
        }
    }

    /// Ends the session with ID `id`, if there is one.
    pub fn end(&self, id: &str) {
        self.live().remove(&hash(id));
    }

    fn within_lifetime(&self, session: &Session, now: DateTime<Utc>) -> bool {
        // A clock that moved backwards gives a negative age, counted as zero.
        let age = (now - session.started).to_std().unwrap_or(Duration::ZERO);
        age < self.lifetime
    }

    /// The session table. Each change to it is a single insert or removal,
    /// so a panic elsewhere cannot leave it half changed, and it stays usable
    /// after one.
    fn live(&self) -> MutexGuard<'_, HashMap<[u8; 32], Session>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn hash(id: &str) -> [u8; 32] {
    Sha256::digest(id.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::testing::FixedClock;
    use crate::names::testing::SeqEntropy;
    use chrono::TimeZone;

    fn sessions(lifetime_secs: u64) -> (Arc<FixedClock>, Sessions) {
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let clock = Arc::new(FixedClock::new(start));
        let sessions = Sessions::new(
            clock.clone(),
            Arc::new(SeqEntropy::new(1)),
            Duration::from_secs(lifetime_secs),
        );
        (clock, sessions)
    }

    #[test]
    fn a_started_session_is_live_until_its_lifetime_ends() {
        let (clock, sessions) = sessions(60);
        let id = sessions.start(Principal::KeyHolder);
        assert_eq!(id.len(), WEB_SESSION_ID_CHARS);
        assert_eq!(sessions.lookup(&id), Lookup::Live(Principal::KeyHolder));
        clock.advance(chrono::Duration::seconds(59));
        assert_eq!(sessions.lookup(&id), Lookup::Live(Principal::KeyHolder));
        clock.advance(chrono::Duration::seconds(1));
        assert_eq!(sessions.lookup(&id), Lookup::Unknown);
    }

    #[test]
    fn unknown_and_ended_sessions_are_not_live() {
        let (_, sessions) = sessions(60);
        assert_eq!(sessions.lookup("never-issued"), Lookup::Unknown);
        let id = sessions.start(Principal::KeyHolder);
        let other = sessions.start(Principal::KeyHolder);
        assert_ne!(id, other);
        sessions.end(&id);
        assert_eq!(sessions.lookup(&id), Lookup::Unknown);
        assert_eq!(sessions.lookup(&other), Lookup::Live(Principal::KeyHolder));
    }

    #[test]
    fn expired_sessions_are_dropped_when_another_starts() {
        let (clock, sessions) = sessions(60);
        sessions.start(Principal::KeyHolder);
        clock.advance(chrono::Duration::seconds(60));
        sessions.start(Principal::KeyHolder);
        assert_eq!(sessions.live().len(), 1);
    }

    #[test]
    fn only_a_hash_of_the_id_is_kept() {
        let (_, sessions) = sessions(60);
        let id = sessions.start(Principal::KeyHolder);
        let live = sessions.live();
        let stored = live.keys().next().expect("one session");
        assert_eq!(stored, &hash(&id));
        assert_ne!(&stored[..], id.as_bytes());
    }
}
