//! Showing the Tailscale login link, and opening it in a browser when the
//! person asks. The asking is behind [`LoginLink`] so setup can be tested
//! without a terminal or a browser.

use std::io::{self, IsTerminal};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What setup does with the login link. [`provision`](super::provision) shows
/// the link once, then asks `open_requested` each time it checks whether
/// the person has logged in, so waiting for a key press never stops it
/// watching for the login.
pub trait LoginLink {
    /// Shows the link to log in with.
    fn show(&self, url: &str);
    /// Whether the person has asked to open the link since the last time this
    /// was asked.
    fn open_requested(&self) -> bool;
    /// Opens the link in a browser.
    fn open(&self, url: &str) -> io::Result<()>;
}

/// Prints the link and offers nothing else: for when no one is at a
/// terminal, such as a script or an agent.
pub struct PrintedLink;

impl LoginLink for PrintedLink {
    fn show(&self, url: &str) {
        println!("Log in to Tailscale by opening: {url}");
    }

    fn open_requested(&self) -> bool {
        false
    }

    fn open(&self, _url: &str) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not asked for here",
        ))
    }
}

/// At a terminal, shows the link and offers to open it when Enter is pressed.
/// Away from a terminal it only prints the link, like [`PrintedLink`]. It
/// never opens a browser unless asked.
pub struct TerminalLink {
    pressed: Arc<AtomicBool>,
}

impl TerminalLink {
    pub fn new() -> TerminalLink {
        TerminalLink {
            pressed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Default for TerminalLink {
    fn default() -> TerminalLink {
        TerminalLink::new()
    }
}

impl LoginLink for TerminalLink {
    fn show(&self, url: &str) {
        if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
            PrintedLink.show(url);
            return;
        }
        println!("Log in to Tailscale by opening this link:\n  {url}");
        println!("Press Enter to open it in your browser (or open it yourself).");
        // A key press is waited for on its own thread, so that setup goes on
        // watching for the login meanwhile. If the person logs in another
        // way, this thread is left waiting; it reads nothing else.
        let pressed = Arc::clone(&self.pressed);
        std::thread::spawn(move || {
            let mut line = String::new();
            if matches!(io::stdin().read_line(&mut line), Ok(n) if n > 0) {
                pressed.store(true, Ordering::SeqCst);
            }
        });
    }

    fn open_requested(&self) -> bool {
        self.pressed.swap(false, Ordering::SeqCst)
    }

    fn open(&self, url: &str) -> io::Result<()> {
        std::process::Command::new(opener())
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    }
}

/// The program that opens a link in the person's browser.
fn opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_opener_is_open_on_a_mac_and_xdg_open_elsewhere() {
        let expected = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(opener(), expected);
    }

    #[test]
    fn a_printed_link_offers_nothing_to_open() {
        assert!(!PrintedLink.open_requested());
        assert!(PrintedLink.open("https://example.com").is_err());
    }

    #[test]
    fn a_terminal_link_has_no_request_until_a_key_is_pressed() {
        let link = TerminalLink::new();
        assert!(!link.open_requested());
        link.pressed.store(true, Ordering::SeqCst);
        assert!(link.open_requested());
        // A request is taken once.
        assert!(!link.open_requested());
    }
}
