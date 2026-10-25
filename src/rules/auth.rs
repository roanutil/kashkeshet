//! Tokens: making them, storing only their hash, spotting them in text, and
//! checking them on each call.

use super::*;
use crate::names::{ALPHABET, random_chars};
use sha2::{Digest, Sha256};

pub const TOKEN_PREFIX: &str = "act_";
pub const TOKEN_RANDOM_LEN: usize = 26;

/// A random token, such as `act_` followed by 26 characters.
pub fn token(entropy: &dyn Entropy) -> String {
    format!("{TOKEN_PREFIX}{}", random_chars(entropy, TOKEN_RANDOM_LEN))
}

/// The hash stored in place of a token.
pub fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// True if `text` contains something shaped like a token.
pub fn contains_token(text: &str) -> bool {
    let bytes = text.as_bytes();
    let prefix = TOKEN_PREFIX.as_bytes();
    let mut start = 0;
    while let Some(offset) = find(&bytes[start..], prefix) {
        let tail = &bytes[start + offset + prefix.len()..];
        if tail.len() >= TOKEN_RANDOM_LEN
            && tail[..TOKEN_RANDOM_LEN]
                .iter()
                .all(|b| ALPHABET.contains(b))
        {
            return true;
        }
        start += offset + 1;
    }
    false
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

impl Rules {
    /// Checks a token and returns the agent it belongs to. Records when the
    /// agent was last seen, at most once per `last_seen_interval`.
    pub async fn authenticate(&self, token: Option<&str>) -> Result<Agent> {
        let token = match token {
            Some(t) if !t.trim().is_empty() => t.trim(),
            _ => return Err(RuleError::unauthenticated()),
        };
        let hash = hash_token(token);
        let found: Option<(String, Option<String>)> = self
            .db()
            .read(move |conn| {
                conn.query_row(
                    "SELECT name, last_seen FROM agents WHERE token_hash = ?",
                    [&hash],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(RuleError::from)
            })
            .await?;
        let Some((name, last_seen)) = found else {
            return Err(RuleError::unauthenticated());
        };

        let now = self.inner.deps.clock.now();
        let interval = chrono::Duration::from_std(self.limits().last_seen_interval)
            .map_err(|e| RuleError::new(Code::Internal, e.to_string()))?;
        let stale = match last_seen.as_deref().and_then(clock::from_text) {
            Some(seen) => now - seen >= interval,
            None => true,
        };
        if stale {
            let who = name.clone();
            let now = clock::to_text(now);
            self.db()
                .write(move |tx| {
                    tx.execute(
                        "UPDATE agents SET last_seen = ? WHERE name = ?",
                        [&now, &who],
                    )?;
                    Ok::<_, RuleError>(())
                })
                .await?;
        }
        Ok(Agent { name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::testing::SeqEntropy;

    #[test]
    fn token_shape() {
        let e = SeqEntropy::new(1);
        let t = token(&e);
        assert!(t.starts_with("act_") && t.len() == 30);
    }

    #[test]
    fn detects_tokens() {
        let e = SeqEntropy::new(7);
        let t = token(&e);
        assert!(contains_token(&t));
        assert!(contains_token(&format!("my token is {t}, keep it")));
        assert!(!contains_token("react_component is fine"));
        assert!(!contains_token("act_short"));
        assert!(!contains_token(""));
    }

    #[test]
    fn hash_is_stable_and_not_the_token() {
        assert_eq!(hash_token("act_x"), hash_token("act_x"));
        assert_ne!(hash_token("act_x"), b"act_x".to_vec());
    }
}
