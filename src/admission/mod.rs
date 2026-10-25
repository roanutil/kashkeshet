//! Admission: whether an HTTP caller is let in to `/mcp` at all. This comes
//! before agent tokens (`rules::auth`) and channel access (`rules::access`),
//! and knows nothing about either.
//!
//! The HTTP layer turns a request into [`Credentials`], asks
//! [`Admission::decide`], and then acts on the [`Decision`]. Each way of
//! admitting callers is one variant of [`Admission`]; a bearer-token
//! validator for OAuth would be another.

mod shared_key;

pub use shared_key::SharedKey;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
