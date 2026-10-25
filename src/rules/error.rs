use crate::store::StoreError;
use serde::Serialize;

/// The fixed error codes agents see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    Unauthenticated,
    NotFound,
    NotMember,
    BodyTooLong,
    ContainsSecret,
    RateLimited,
    /// A server-wide limit, such as the most agents, has been reached.
    LimitReached,
    InvalidArgument,
    Internal,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Unauthenticated => "unauthenticated",
            Code::NotFound => "not_found",
            Code::NotMember => "not_member",
            Code::BodyTooLong => "body_too_long",
            Code::ContainsSecret => "contains_secret",
            Code::RateLimited => "rate_limited",
            Code::LimitReached => "limit_reached",
            Code::InvalidArgument => "invalid_argument",
            Code::Internal => "internal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct RuleError {
    pub code: Code,
    pub message: String,
}

impl RuleError {
    pub fn new(code: Code, message: impl Into<String>) -> RuleError {
        RuleError {
            code,
            message: message.into(),
        }
    }

    pub fn unauthenticated() -> RuleError {
        RuleError::new(
            Code::Unauthenticated,
            "missing or unknown token. Call `register` (no arguments) to get a name and token, \
             then pass `token` on every other call.",
        )
    }

    pub fn channel_not_found(id: &str) -> RuleError {
        RuleError::new(Code::NotFound, format!("no channel {id}"))
    }

    pub fn agent_not_found(name: &str) -> RuleError {
        RuleError::new(Code::NotFound, format!("no agent named {name}"))
    }

    pub fn message_not_found(id: i64) -> RuleError {
        RuleError::new(Code::NotFound, format!("no message {id}"))
    }

    pub fn not_member(id: &str) -> RuleError {
        RuleError::new(
            Code::NotMember,
            format!("you are not a member of {id}. It is open: call `join_channel` first."),
        )
    }

    pub fn invalid(message: impl Into<String>) -> RuleError {
        RuleError::new(Code::InvalidArgument, message)
    }
}

impl From<StoreError> for RuleError {
    fn from(e: StoreError) -> RuleError {
        RuleError::new(Code::Internal, e.to_string())
    }
}
