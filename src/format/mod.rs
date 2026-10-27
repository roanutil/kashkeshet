//! Turns rule results into small responses: plain-text lines by default, JSON
//! on request. Lists and message previews fit the response size cap. Single-item
//! get_message, registration and identity replies keep their full contents.
//! When something is left out, the response says how to get it.
//!
//! `fit` holds the size-cap machinery every response shares; the other files
//! hold the output for one area each.

mod agents;
mod channels;
mod fit;
mod messages;

#[cfg(test)]
mod tests;

use crate::clock::Clock;
use crate::rules::RuleError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Compact lines of text. The default.
    #[default]
    Text,
    /// A JSON object.
    Json,
}

#[derive(Clone)]
pub struct Formatter {
    max_bytes: usize,
    clock: Arc<dyn Clock>,
}

impl Formatter {
    pub fn new(max_response_bytes: usize, clock: Arc<dyn Clock>) -> Formatter {
        Formatter {
            max_bytes: max_response_bytes,
            clock,
        }
    }

    pub fn error(&self, f: Format, e: &RuleError) -> String {
        match f {
            Format::Text => format!("error: {}: {}", e.code.as_str(), e.message),
            Format::Json => {
                json!({ "error": { "code": e.code, "message": e.message } }).to_string()
            }
        }
    }

    pub fn ok(&self, f: Format) -> String {
        match f {
            Format::Text => "ok".to_string(),
            Format::Json => json!({ "ok": true }).to_string(),
        }
    }
}
