//! Tokens: making them, storing only their hash, spotting them in text, and
//! checking them on each call.

use super::*;
use crate::names::{ALPHABET, random_chars};
use sha2::{Digest, Sha256};

use crate::config::{AGENT_SECRET_CHARS, RECOVERY_PREFIX, TOKEN_PREFIX};

/// A random token, such as `act_` followed by 26 characters.
pub fn token(entropy: &dyn Entropy) -> String {
    format!(
        "{TOKEN_PREFIX}{}",
        random_chars(entropy, AGENT_SECRET_CHARS)
    )
}

/// A recovery code, issued once when an agent registers.
pub fn recovery_code(entropy: &dyn Entropy) -> String {
    format!(
        "{RECOVERY_PREFIX}{}",
        random_chars(entropy, AGENT_SECRET_CHARS)
    )
}

/// The hash stored in place of a token or recovery code.
pub fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// True if `text` contains something shaped like a token or recovery code.
pub fn contains_secret(text: &str) -> bool {
    [TOKEN_PREFIX, RECOVERY_PREFIX]
        .iter()
        .any(|prefix| contains_shaped_secret(text, prefix))
}

fn contains_shaped_secret(text: &str, prefix: &str) -> bool {
    let bytes = text.as_bytes();
    let prefix = prefix.as_bytes();
    let mut start = 0;
    while let Some(offset) = find(&bytes[start..], prefix) {
        let tail = &bytes[start + offset + prefix.len()..];
        if tail.len() >= AGENT_SECRET_CHARS
            && tail[..AGENT_SECRET_CHARS]
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
    /// Checks a token and returns the agent it belongs to. Records the time of every
    /// authenticated call, including calls whose arguments are later refused.
    pub async fn authenticate(&self, token: Option<&str>) -> Result<Agent> {
        let token = match token {
            Some(t) if !t.trim().is_empty() => t.trim(),
            _ => return Err(RuleError::unauthenticated()),
        };
        let hash = hash_token(token);
        let found = self
            .db()
            .read(move |store| store.agent_by_token_hash(&hash).map_err(RuleError::from))
            .await?;
        // A revoked token is refused exactly like one that was never issued.
        let Some(AgentRecord {
            name,
            revoked_at: None,
            ..
        }) = found
        else {
            return Err(RuleError::unauthenticated());
        };

        let now = self.inner.deps.clock.now();
        let who = name.clone();
        let now = clock::to_text(now);
        self.db()
            .write(move |store| {
                store.set_last_seen(&who, &now)?;
                Ok::<_, RuleError>(())
            })
            .await?;
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
        assert!(contains_secret(&t));
        assert!(contains_secret(&format!("my token is {t}, keep it")));
        assert!(!contains_secret("react_component is fine"));
        assert!(!contains_secret("act_short"));
        assert!(!contains_secret(""));
    }

    #[test]
    fn detects_recovery_codes_without_confusing_ordinary_text() {
        let code = recovery_code(&SeqEntropy::new(9));
        assert!(code.starts_with("arc_") && code.len() == 30);
        assert!(contains_secret(&format!("秘密 {code}, private")));
        assert!(contains_secret(&format!("act_short then {code}")));
        assert!(!contains_secret("arc_short"));
        assert!(!contains_secret("arc_!!!!!!!!!!!!!!!!!!!!!!!!!!"));
        assert!(!contains_secret("an arc in a diagram"));
    }

    #[test]
    fn hash_is_stable_and_not_the_token() {
        assert_eq!(hash_token("act_x"), hash_token("act_x"));
        assert_ne!(hash_token("act_x"), b"act_x".to_vec());
    }
}
