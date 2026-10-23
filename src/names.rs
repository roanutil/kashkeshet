//! Agent names, channel IDs, and tokens.
//!
//! Randomness is injected through [`Entropy`] so the rest of the system can be
//! tested with a predictable source.

use sha2::{Digest, Sha256};

/// A source of random numbers.
pub trait Entropy: Send + Sync {
    /// Returns a uniformly chosen number in `0..n`. `n` is never zero.
    fn below(&self, n: usize) -> usize;
}

/// Entropy from the operating system's cryptographic generator.
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn below(&self, n: usize) -> usize {
        use rand::RngExt;
        rand::rng().random_range(0..n)
    }
}

/// Characters used in channel IDs and tokens.
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

pub const TOKEN_PREFIX: &str = "act_";
pub const TOKEN_RANDOM_LEN: usize = 26;
pub const CHANNEL_PREFIX: &str = "ch_";
pub const CHANNEL_RANDOM_LEN: usize = 8;

const ADJECTIVES: &[&str] = &[
    "amber", "ashen", "bold", "brave", "brisk", "calm", "clever", "copper", "coral", "crisp",
    "dapper", "eager", "early", "fair", "fierce", "gentle", "gilded", "glad", "golden", "grand",
    "hazel", "hidden", "honest", "humble", "icy", "ivory", "jade", "jolly", "keen", "kind",
    "lively", "lucky", "lunar", "mellow", "misty", "noble", "olive", "patient", "plucky", "polar",
    "proud", "quick", "quiet", "rapid", "rosy", "rustic", "sandy", "scarlet", "silent", "silver",
    "slate", "sly", "snowy", "solar", "steady", "stormy", "sunny", "swift", "tawny", "tidy",
    "velvet", "vivid", "wise", "witty",
];

const ANIMALS: &[&str] = &[
    "badger", "bear", "beaver", "bison", "bobcat", "crane", "crow", "deer", "dingo", "dolphin",
    "eagle", "egret", "elk", "falcon", "ferret", "finch", "fox", "gecko", "gibbon", "goose",
    "hare", "hawk", "heron", "ibis", "jackal", "jaguar", "koala", "lark", "lemur", "lion", "lynx",
    "magpie", "marten", "mink", "moose", "newt", "ocelot", "orca", "osprey", "otter", "owl",
    "panda", "panther", "parrot", "pelican", "puffin", "quail", "raven", "robin", "salmon", "seal",
    "shrike", "sparrow", "stoat", "swan", "tapir", "tern", "tiger", "toucan", "trout", "turtle",
    "viper", "walrus", "wren",
];

// Tests tell name draws from ID draws by list length; see `testing::SameNameEntropy`.
const _: () = assert!(ADJECTIVES.len() > ALPHABET.len() && ANIMALS.len() > ALPHABET.len());

/// A random adjective-animal pair, such as `amber-falcon`.
pub fn agent_name(entropy: &dyn Entropy) -> String {
    let adjective = ADJECTIVES[entropy.below(ADJECTIVES.len())];
    let animal = ANIMALS[entropy.below(ANIMALS.len())];
    format!("{adjective}-{animal}")
}

/// The name to try after `base` clashes `attempt` times. Attempt 1 gives
/// `base-2`, attempt 2 gives `base-3`, and so on.
pub fn numbered_name(base: &str, attempt: u32) -> String {
    format!("{base}-{}", attempt + 1)
}

/// A random channel ID, such as `ch_7k2m9q4x`.
pub fn channel_id(entropy: &dyn Entropy) -> String {
    format!(
        "{CHANNEL_PREFIX}{}",
        random_chars(entropy, CHANNEL_RANDOM_LEN)
    )
}

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

fn random_chars(entropy: &dyn Entropy, len: usize) -> String {
    (0..len)
        .map(|_| ALPHABET[entropy.below(ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
pub mod testing {
    use super::Entropy;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A predictable entropy source for tests (a simple linear congruential generator).
    pub struct SeqEntropy(AtomicU64);

    impl SeqEntropy {
        pub fn new(seed: u64) -> Self {
            Self(AtomicU64::new(seed))
        }
    }

    impl Entropy for SeqEntropy {
        fn below(&self, n: usize) -> usize {
            let next = self
                .0
                .load(Ordering::Relaxed)
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0.store(next, Ordering::Relaxed);
            ((next >> 33) as usize) % n
        }
    }

    /// Picks the first word for every agent name, so every name clashes, but
    /// draws IDs and tokens normally. Name words come from lists longer than
    /// the ID alphabet, which is how the two kinds of draw are told apart.
    pub struct SameNameEntropy(pub SeqEntropy);

    impl Entropy for SameNameEntropy {
        fn below(&self, n: usize) -> usize {
            if n > super::ALPHABET.len() {
                0
            } else {
                self.0.below(n)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testing::SeqEntropy;

    #[test]
    fn shapes() {
        let e = SeqEntropy::new(1);
        let name = agent_name(&e);
        assert_eq!(name.split('-').count(), 2);
        let id = channel_id(&e);
        assert!(id.starts_with("ch_") && id.len() == 11);
        let t = token(&e);
        assert!(t.starts_with("act_") && t.len() == 30);
        assert_eq!(numbered_name("amber-falcon", 1), "amber-falcon-2");
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
