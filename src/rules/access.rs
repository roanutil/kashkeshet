//! Who may use a channel: the rules table from the module docs, as one pure
//! function.

/// What a call needs from a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Need {
    /// To see it: list its members, or join it.
    See,
    /// To be in it: read, send, set the purpose, add others, leave.
    Membership,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Allowed,
    /// The channel does not exist, or is closed and the caller is not in it.
    /// The two must look the same.
    NotFound,
    /// The channel is open and the caller is not in it.
    NotMember,
}

pub(super) fn decide(exists: bool, open: bool, member: bool, need: Need) -> Decision {
    if !exists || (!open && !member) {
        return Decision::NotFound;
    }
    match (need, member) {
        (Need::Membership, false) => Decision::NotMember,
        _ => Decision::Allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::Decision::*;
    use super::Need::*;
    use super::*;

    #[test]
    fn decisions() {
        // (exists, open, member, need) -> decision
        let table = [
            (false, false, false, See, NotFound),
            (false, false, false, Membership, NotFound),
            (true, true, false, See, Allowed),
            (true, true, false, Membership, NotMember),
            (true, true, true, See, Allowed),
            (true, true, true, Membership, Allowed),
            (true, false, false, See, NotFound),
            (true, false, false, Membership, NotFound),
            (true, false, true, See, Allowed),
            (true, false, true, Membership, Allowed),
        ];
        for (exists, open, member, need, want) in table {
            assert_eq!(
                decide(exists, open, member, need),
                want,
                "exists={exists} open={open} member={member} need={need:?}"
            );
        }
    }
}
