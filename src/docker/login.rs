//! The contract for showing the Tailscale login link. Showing it, and
//! opening it in a browser when the person asks, happen at the edge of the
//! program, so setup can be tested without a terminal or a browser.

use std::io;

/// What setup does with the login link. [`provision`](super::provision) shows
/// the link once, then at each check for the login asks `open_requested` and
/// `open_failure`, so waiting for a key press never stops it watching for the
/// login.
pub trait LoginLink {
    /// Shows the link to log in with.
    fn show(&self, url: &str);
    /// Whether the person has asked to open the link since the last time this
    /// was asked.
    fn open_requested(&self) -> bool;
    /// Starts opening the link in a browser. Fails if it cannot be started;
    /// whether it then worked is told by `open_failure`.
    fn open(&self, url: &str) -> io::Result<()>;
    /// Why the browser opener ended without success, if it did since the
    /// last time this was asked.
    fn open_failure(&self) -> Option<String>;
}
