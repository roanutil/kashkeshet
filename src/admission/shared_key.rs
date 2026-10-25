//! One key shared by every caller, kept only as a hash.

use crate::config::{self, InvalidSetting, NEW_ACCESS_KEY_CHARS};
use crate::names::{Entropy, random_chars};
use sha2::{Digest, Sha256};

/// The shared access key. It keeps a SHA-256 of the key, not the key, and
/// where the key came from.
pub struct SharedKey {
    digest: [u8; 32],
    source: String,
}

impl SharedKey {
    /// The key in `text`, without surrounding whitespace (so a file's final
    /// newline is not part of it). `source` says where it came from, such as
    /// the file path. Fails if the key is not one `config::check_access_key`
    /// accepts.
    pub fn new(text: &str, source: impl Into<String>) -> Result<SharedKey, InvalidSetting> {
        let key = text.trim();
        config::check_access_key(key)?;
        Ok(SharedKey {
            digest: Sha256::digest(key.as_bytes()).into(),
            source: source.into(),
        })
    }

    /// A new random key, such as `kkt key create` writes. It passes
    /// `config::check_access_key`.
    pub fn generate(entropy: &dyn Entropy) -> String {
        random_chars(entropy, NEW_ACCESS_KEY_CHARS)
    }

    /// Where the key came from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether `presented` is the key. Takes the same time whatever
    /// `presented` is: both sides are hashed to the same length, and every
    /// byte is compared.
    pub fn matches(&self, presented: &[u8]) -> bool {
        let presented: [u8; 32] = Sha256::digest(presented).into();
        let diff = self
            .digest
            .iter()
            .zip(presented.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        std::hint::black_box(diff) == 0
    }
}

impl std::fmt::Debug for SharedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedKey")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn surrounding_whitespace_is_not_part_of_the_key() {
        let key = SharedKey::new(&format!("  {KEY}\n"), "file").expect("key");
        assert!(key.matches(KEY.as_bytes()));
        assert!(!key.matches(format!("{KEY}\n").as_bytes()));
    }

    #[test]
    fn keys_config_refuses_are_refused() {
        for text in ["", "\n", "short", &format!("{KEY} {KEY}")] {
            let err = SharedKey::new(text, "file").expect_err(text);
            assert_eq!(err.name, "access_key_file");
        }
    }

    #[test]
    fn generated_keys_are_accepted_and_differ() {
        use crate::names::testing::SeqEntropy;
        let entropy = SeqEntropy::new(3);
        let first = SharedKey::generate(&entropy);
        let second = SharedKey::generate(&entropy);
        assert_eq!(first.len(), NEW_ACCESS_KEY_CHARS);
        assert_ne!(first, second);
        let key = SharedKey::new(&first, "file").expect("a generated key is valid");
        assert!(key.matches(first.as_bytes()));
    }

    #[test]
    fn debug_does_not_show_the_key() {
        let key = SharedKey::new(KEY, "/keys/kkt").expect("key");
        let shown = format!("{key:?}");
        assert!(shown.contains("/keys/kkt"));
        assert!(!shown.contains(KEY));
    }
}
