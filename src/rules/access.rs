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

/// Who is asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Who {
    /// An agent, and whether it is in the channel.
    Agent { member: bool },
    /// The human operator, who sees every channel and may change any of
    /// them without being in it.
    Human,
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

pub(super) fn decide(exists: bool, open: bool, who: Who, need: Need) -> Decision {
    let member = match who {
        Who::Human => {
            return if exists {
                Decision::Allowed
            } else {
                Decision::NotFound
            };
        }
        Who::Agent { member } => member,
    };
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
        let agent = |member| Who::Agent { member };
        // (exists, open, who, need) -> decision
        let table = [
            (false, false, agent(false), See, NotFound),
            (false, false, agent(false), Membership, NotFound),
            (true, true, agent(false), See, Allowed),
            (true, true, agent(false), Membership, NotMember),
            (true, true, agent(true), See, Allowed),
            (true, true, agent(true), Membership, Allowed),
            (true, false, agent(false), See, NotFound),
            (true, false, agent(false), Membership, NotFound),
            (true, false, agent(true), See, Allowed),
            (true, false, agent(true), Membership, Allowed),
            (false, false, Who::Human, See, NotFound),
            (false, true, Who::Human, Membership, NotFound),
            (true, true, Who::Human, See, Allowed),
            (true, true, Who::Human, Membership, Allowed),
            (true, false, Who::Human, See, Allowed),
            (true, false, Who::Human, Membership, Allowed),
        ];
        for (exists, open, who, need, want) in table {
            assert_eq!(
                decide(exists, open, who, need),
                want,
                "exists={exists} open={open} who={who:?} need={need:?}"
            );
        }
    }
}
