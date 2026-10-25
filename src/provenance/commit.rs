//! How the build names the git commit it was built from. `build.rs` includes
//! this file, so it must not use anything outside the standard library.

/// The commit text stored with each server run: the commit hash, with
/// `-dirty` added when the working tree had uncommitted changes. `unknown`
/// when git could not say, as when building outside a git checkout.
pub fn commit_text(commit: Option<&str>, dirty: Option<bool>) -> String {
    match (commit.map(str::trim), dirty) {
        (Some(c), Some(true)) if !c.is_empty() => format!("{c}-dirty"),
        (Some(c), _) if !c.is_empty() => c.to_string(),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_checkout_gives_the_hash() {
        assert_eq!(commit_text(Some("abc123\n"), Some(false)), "abc123");
    }

    #[test]
    fn uncommitted_changes_are_marked() {
        assert_eq!(commit_text(Some("abc123"), Some(true)), "abc123-dirty");
    }

    #[test]
    fn without_git_the_commit_is_unknown() {
        assert_eq!(commit_text(None, None), "unknown");
        assert_eq!(commit_text(None, Some(true)), "unknown");
        assert_eq!(commit_text(Some("  "), Some(false)), "unknown");
    }

    #[test]
    fn a_hash_without_a_dirty_answer_is_kept() {
        assert_eq!(commit_text(Some("abc123"), None), "abc123");
    }
}
