//! Whether the installed binary should replace the recorded server version.

use std::cmp::Ordering;

#[derive(Debug, PartialEq, Eq)]
pub enum Upgrade {
    Already,
    Proceed,
}

/// Compare semantic versions without exposing the parser's types. Build
/// metadata does not affect precedence. Unknown image tags require the same
/// explicit override as a downgrade, because their ordering is unknowable.
pub fn upgrade(current: &str, target: &str, allow_downgrade: bool) -> Result<Upgrade, String> {
    let target_version = semver::Version::parse(target)
        .map_err(|_| format!("this kkt has an invalid version {target:?}"))?;
    let current_version = match semver::Version::parse(current) {
        Ok(version) => version,
        Err(_) if allow_downgrade => return Ok(Upgrade::Proceed),
        Err(_) => {
            return Err(format!(
                "cannot compare server version {current:?} with {target}; pass --allow-downgrade to replace it explicitly"
            ));
        }
    };
    match target_version.cmp_precedence(&current_version) {
        Ordering::Equal => Ok(Upgrade::Already),
        Ordering::Greater => Ok(Upgrade::Proceed),
        Ordering::Less if allow_downgrade => Ok(Upgrade::Proceed),
        Ordering::Less => Err(format!(
            "refusing to downgrade the server from {current} to {target}; pass --allow-downgrade to override"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_version_including_build_metadata_does_nothing() {
        for (current, target) in [("0.4.0", "0.4.0"), ("0.4.0+one", "0.4.0+two")] {
            assert_eq!(upgrade(current, target, false).unwrap(), Upgrade::Already);
            assert_eq!(upgrade(current, target, true).unwrap(), Upgrade::Already);
        }
    }

    #[test]
    fn upgrades_use_numeric_and_prerelease_precedence() {
        for (current, target) in [
            ("0.9.0", "0.10.0"),
            ("0.4.0-rc.9", "0.4.0-rc.10"),
            ("0.4.0-rc.1", "0.4.0"),
        ] {
            assert_eq!(upgrade(current, target, false).unwrap(), Upgrade::Proceed);
            assert!(
                upgrade(target, current, false)
                    .unwrap_err()
                    .contains("--allow-downgrade")
            );
            assert_eq!(upgrade(target, current, true).unwrap(), Upgrade::Proceed);
        }
    }

    #[test]
    fn unknown_tags_require_override_and_invalid_target_is_always_refused() {
        assert!(
            upgrade("local-test", "0.4.0", false)
                .unwrap_err()
                .contains("cannot compare")
        );
        assert_eq!(
            upgrade("local-test", "0.4.0", true).unwrap(),
            Upgrade::Proceed
        );
        assert!(upgrade("0.4.0", "invalid", true).is_err());
    }
}
