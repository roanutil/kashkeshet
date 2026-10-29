//! Admission: whether an HTTP caller is let in to `/mcp` or the web page at
//! all. This comes before agent tokens (`rules::auth`) and channel access
//! (`rules::access`), and knows nothing about either.
//!
//! For `/mcp`, the HTTP layer turns a request into [`Credentials`], asks
//! [`Admission::decide`], and then acts on the [`Decision`]. Each way of
//! admitting callers is one variant of [`Admission`]; a bearer-token
//! validator for OAuth would be another.
//!
//! The web page asks [`WebAdmission`] instead. A viewer signs in once with
//! credentials [`Admission`] accepts and gets a session; later requests are
//! let in by the session alone.

mod session;
mod shared_key;

pub use session::Sessions;
pub use shared_key::SharedKey;

use session::Lookup;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;

/// How callers are admitted.
pub enum Admission {
    /// Everyone is let in. The server's behavior before admission existed.
    Open,
    /// Callers must present one shared key as a bearer token.
    SharedKey(SharedKey),
}

/// What a caller presented to be let in, read from its `Authorization`
/// header. Holds a secret, so it has no `Debug`.
pub struct Credentials {
    presented: Presented,
}

enum Presented {
    Nothing,
    Bearer(Vec<u8>),
    /// A header that is not a bearer token, such as `Basic ...`.
    Other,
}

impl Credentials {
    /// Reads the value of an `Authorization` header, if the request had one.
    /// The scheme name `Bearer` is matched without regard to case.
    pub fn from_authorization(header: Option<&[u8]>) -> Credentials {
        const SCHEME: &[u8] = b"bearer ";
        let presented = match header {
            None => Presented::Nothing,
            Some(value)
                if value.len() > SCHEME.len()
                    && value[..SCHEME.len()].eq_ignore_ascii_case(SCHEME) =>
            {
                Presented::Bearer(value[SCHEME.len()..].trim_ascii().to_vec())
            }
            Some(_) => Presented::Other,
        };
        Credentials { presented }
    }

    /// A key typed into a sign-in form, without surrounding whitespace.
    pub fn from_key(key: &str) -> Credentials {
        Credentials {
            presented: Presented::Bearer(key.trim().as_bytes().to_vec()),
        }
    }
}

/// The answer for one caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Admitted(Principal),
    Refused(Refusal),
}

/// Who an admitted caller is, as far as admission can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Principal {
    /// No admission check is set, so the caller is not known.
    Anyone,
    /// The caller presented the shared key.
    KeyHolder,
}

/// Why a caller was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("an access key is required: send Authorization: Bearer <key>")]
    NoCredentials,
    #[error("the Authorization header must be a bearer token: Bearer <key>")]
    NotBearer,
    #[error("the access key is wrong")]
    WrongKey,
    #[error("sign in first")]
    NoSession,
    #[error("the session has ended; sign in again")]
    SessionEnded,
}

impl Admission {
    /// Decides whether the caller with `credentials` is let in.
    pub fn decide(&self, credentials: &Credentials) -> Decision {
        match self {
            Admission::Open => Decision::Admitted(Principal::Anyone),
            Admission::SharedKey(key) => match &credentials.presented {
                Presented::Nothing => Decision::Refused(Refusal::NoCredentials),
                Presented::Other => Decision::Refused(Refusal::NotBearer),
                Presented::Bearer(token) if key.matches(token) => {
                    Decision::Admitted(Principal::KeyHolder)
                }
                Presented::Bearer(_) => Decision::Refused(Refusal::WrongKey),
            },
        }
    }

    /// Where the shared key came from, for the record of a server run.
    /// `None` when no key is set. Never the key itself.
    pub fn key_source(&self) -> Option<&str> {
        match self {
            Admission::Open => None,
            Admission::SharedKey(key) => Some(key.source()),
        }
    }
}

/// Admission for the web page. With [`Admission::Open`], everyone sees it.
/// Otherwise a viewer needs a live session.
pub struct WebAdmission {
    admission: Arc<Admission>,
    sessions: Sessions,
    /// The form token when no sign-in is required, so there are no
    /// sessions to tie one to. Drawn at startup; a restart changes it.
    open_form_token: String,
}

/// The answer to a sign-in.
pub enum SignIn {
    /// Signed in. `session_id` goes back to the viewer and nowhere else; the
    /// session lasts `lifetime`.
    Started {
        session_id: String,
        lifetime: Duration,
    },
    Refused(Refusal),
}

impl WebAdmission {
    pub fn new(admission: Arc<Admission>, sessions: Sessions) -> WebAdmission {
        WebAdmission {
            admission,
            open_form_token: sessions.secret(),
            sessions,
        }
    }

    /// The token a page puts in its forms, so a change can be told from a
    /// request forged by another site, which cannot read the page. With
    /// sign-in, it belongs to the viewer's live session (a hash of the
    /// session ID, which only the viewer's cookie holds); `None` without one.
    /// Without sign-in, it is one token for this server run.
    pub fn form_token(&self, session_id: Option<&str>) -> Option<String> {
        if !self.sign_in_required() {
            return Some(self.open_form_token.clone());
        }
        let id = session_id?;
        match self.sessions.lookup(id) {
            Lookup::Live(_) => {
                let digest = Sha256::digest(format!("kkt-form:{id}").as_bytes());
                Some(digest.iter().map(|b| format!("{b:02x}")).collect())
            }
            Lookup::Unknown => None,
        }
    }

    /// Whether `presented` is the form token for the viewer holding
    /// `session_id`. Takes the same time whatever `presented` is.
    pub fn form_token_matches(&self, session_id: Option<&str>, presented: &str) -> bool {
        let Some(expected) = self.form_token(session_id) else {
            return false;
        };
        let expected: [u8; 32] = Sha256::digest(expected.as_bytes()).into();
        let presented: [u8; 32] = Sha256::digest(presented.as_bytes()).into();
        let diff = expected
            .iter()
            .zip(presented.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        std::hint::black_box(diff) == 0
    }

    /// Whether viewers must sign in. When false, `decide` admits everyone.
    pub fn sign_in_required(&self) -> bool {
        !matches!(*self.admission, Admission::Open)
    }

    /// Decides whether the viewer holding `session_id`, if any, is let in.
    pub fn decide(&self, session_id: Option<&str>) -> Decision {
        if !self.sign_in_required() {
            return Decision::Admitted(Principal::Anyone);
        }
        match session_id.map(|id| self.sessions.lookup(id)) {
            None => Decision::Refused(Refusal::NoSession),
            Some(Lookup::Live(principal)) => Decision::Admitted(principal),
            Some(Lookup::Unknown) => Decision::Refused(Refusal::SessionEnded),
        }
    }

    /// Starts a session if `credentials` match the web page's own key.
    pub fn sign_in(&self, credentials: &Credentials) -> SignIn {
        match self.admission.decide(credentials) {
            Decision::Admitted(principal) => SignIn::Started {
                session_id: self.sessions.start(principal),
                lifetime: self.sessions.lifetime(),
            },
            Decision::Refused(refusal) => SignIn::Refused(refusal),
        }
    }

    /// Ends the session with `session_id`, if there is one.
    pub fn sign_out(&self, session_id: &str) {
        self.sessions.end(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::testing::FixedClock;
    use crate::names::testing::SeqEntropy;
    use chrono::{TimeZone, Utc};

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn keyed() -> Admission {
        Admission::SharedKey(SharedKey::new(KEY, "/keys/kkt").expect("key"))
    }

    fn decide(admission: &Admission, header: Option<&str>) -> Decision {
        admission.decide(&Credentials::from_authorization(header.map(str::as_bytes)))
    }

    #[test]
    fn open_admits_everyone() {
        let open = Admission::Open;
        for header in [None, Some("Bearer nope"), Some("Basic eA==")] {
            assert_eq!(decide(&open, header), Decision::Admitted(Principal::Anyone));
        }
        assert_eq!(open.key_source(), None);
    }

    #[test]
    fn the_right_key_is_admitted() {
        let bearer = format!("Bearer {KEY}");
        assert_eq!(
            decide(&keyed(), Some(&bearer)),
            Decision::Admitted(Principal::KeyHolder)
        );
        let lower = format!("bearer  {KEY} ");
        assert_eq!(
            decide(&keyed(), Some(&lower)),
            Decision::Admitted(Principal::KeyHolder)
        );
    }

    #[test]
    fn a_missing_or_wrong_key_is_refused() {
        let admission = keyed();
        assert_eq!(
            decide(&admission, None),
            Decision::Refused(Refusal::NoCredentials)
        );
        assert_eq!(
            decide(&admission, Some("Bearer 0123456789abcdef0123456789abcdeX")),
            Decision::Refused(Refusal::WrongKey)
        );
        assert_eq!(
            decide(&admission, Some("Bearer ")),
            Decision::Refused(Refusal::NotBearer)
        );
        let prefix = format!("Bearer {}", &KEY[..10]);
        assert_eq!(
            decide(&admission, Some(&prefix)),
            Decision::Refused(Refusal::WrongKey)
        );
        let basic = format!("Basic {KEY}");
        assert_eq!(
            decide(&admission, Some(&basic)),
            Decision::Refused(Refusal::NotBearer)
        );
    }

    #[test]
    fn the_key_source_is_recorded_but_not_the_key() {
        assert_eq!(keyed().key_source(), Some("/keys/kkt"));
    }

    fn web(admission: Admission) -> (Arc<FixedClock>, WebAdmission) {
        web_with_seed(admission, 7)
    }

    fn web_with_seed(admission: Admission, seed: u64) -> (Arc<FixedClock>, WebAdmission) {
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let clock = Arc::new(FixedClock::new(start));
        let sessions = Sessions::new(
            clock.clone(),
            Arc::new(SeqEntropy::new(seed)),
            Duration::from_secs(3600),
        );
        (clock, WebAdmission::new(Arc::new(admission), sessions))
    }

    fn started(sign_in: SignIn) -> String {
        match sign_in {
            SignIn::Started { session_id, .. } => session_id,
            SignIn::Refused(r) => panic!("refused: {r}"),
        }
    }

    #[test]
    fn an_open_web_page_needs_no_session() {
        let (_, web) = web(Admission::Open);
        assert!(!web.sign_in_required());
        assert_eq!(web.decide(None), Decision::Admitted(Principal::Anyone));
    }

    #[test]
    fn signing_in_with_the_key_gives_a_session_that_expires() {
        let (clock, web) = web(keyed());
        assert!(web.sign_in_required());
        assert_eq!(web.decide(None), Decision::Refused(Refusal::NoSession));
        let id = started(web.sign_in(&Credentials::from_key(&format!(" {KEY}\n"))));
        assert_eq!(
            web.decide(Some(&id)),
            Decision::Admitted(Principal::KeyHolder)
        );
        clock.advance(chrono::Duration::seconds(3600));
        assert_eq!(
            web.decide(Some(&id)),
            Decision::Refused(Refusal::SessionEnded)
        );
    }

    #[test]
    fn a_wrong_key_gives_no_session() {
        let (_, web) = web(keyed());
        assert!(matches!(
            web.sign_in(&Credentials::from_key("0123456789abcdef0123456789abcdeX")),
            SignIn::Refused(Refusal::WrongKey)
        ));
        assert_eq!(
            web.decide(Some("guess")),
            Decision::Refused(Refusal::SessionEnded)
        );
    }

    #[test]
    fn form_tokens_belong_to_a_live_session() {
        let (clock, web) = web(keyed());
        assert_eq!(web.form_token(None), None);
        assert_eq!(web.form_token(Some("guess")), None);
        let id = started(web.sign_in(&Credentials::from_key(KEY)));
        let other = started(web.sign_in(&Credentials::from_key(KEY)));
        let token = web.form_token(Some(&id)).expect("a token");
        assert!(
            !token.contains(&id),
            "the token does not reveal the session"
        );
        assert!(web.form_token_matches(Some(&id), &token));
        // Another session's token, a wrong one, or none at all is refused.
        let others = web.form_token(Some(&other)).expect("a token");
        assert_ne!(token, others);
        assert!(!web.form_token_matches(Some(&id), &others));
        assert!(!web.form_token_matches(Some(&id), ""));
        assert!(!web.form_token_matches(None, &token));
        // It ends with its session.
        web.sign_out(&other);
        assert!(!web.form_token_matches(Some(&other), &others));
        clock.advance(chrono::Duration::seconds(3600));
        assert!(!web.form_token_matches(Some(&id), &token));
    }

    #[test]
    fn an_open_web_page_has_one_form_token_per_run() {
        let (_, web) = web(Admission::Open);
        let token = web.form_token(None).expect("a token");
        assert_eq!(token.len(), crate::config::WEB_SESSION_ID_CHARS);
        assert_eq!(web.form_token(Some("anything")), Some(token.clone()));
        assert!(web.form_token_matches(None, &token));
        assert!(!web.form_token_matches(None, "wrong"));
        assert!(!web.form_token_matches(None, ""));
        let (_, restarted) = web_with_seed(Admission::Open, 8);
        assert_ne!(restarted.form_token(None), Some(token));
    }

    #[test]
    fn signing_out_ends_the_session() {
        let (_, web) = web(keyed());
        let id = started(web.sign_in(&Credentials::from_key(KEY)));
        web.sign_out(&id);
        assert_eq!(
            web.decide(Some(&id)),
            Decision::Refused(Refusal::SessionEnded)
        );
    }
}
