//! Agent names and channel IDs, and the random characters that IDs and
//! tokens are made of.
//!
//! Randomness is injected through [`Entropy`] so the rest of the system can be
//! tested with a predictable source.

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

/// Entropy that repeats from a seed: the same seed gives the same numbers in
/// the same order. For simulations such as the web preview; never for tokens
/// or keys. (SplitMix64.)
pub struct SeededEntropy(std::sync::atomic::AtomicU64);

impl SeededEntropy {
    pub fn new(seed: u64) -> SeededEntropy {
        SeededEntropy(std::sync::atomic::AtomicU64::new(seed))
    }
}

impl Entropy for SeededEntropy {
    fn below(&self, n: usize) -> usize {
        const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
        let state = self
            .0
            .fetch_add(GAMMA, std::sync::atomic::Ordering::Relaxed)
            .wrapping_add(GAMMA);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z % n as u64) as usize
    }
}

/// Characters used in channel IDs and tokens.
pub(crate) const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

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

pub(crate) fn random_chars(entropy: &dyn Entropy, len: usize) -> String {
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
    fn seeded_entropy_repeats_from_its_seed() {
        let draws = |seed| {
            let e = SeededEntropy::new(seed);
            (0..20).map(|_| e.below(1000)).collect::<Vec<_>>()
        };
        assert_eq!(draws(7), draws(7));
        assert_ne!(draws(7), draws(8));
        assert!(draws(7).iter().all(|&n| n < 1000));
    }

    #[test]
    fn shapes() {
        let e = SeqEntropy::new(1);
        let name = agent_name(&e);
        assert_eq!(name.split('-').count(), 2);
        let id = channel_id(&e);
        assert!(id.starts_with("ch_") && id.len() == 11);
        assert_eq!(numbered_name("amber-falcon", 1), "amber-falcon-2");
    }
}
