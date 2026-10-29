//! The terminal's part of the Tailscale login: showing the link, offering to
//! open it in a browser when Enter is pressed, and opening it with the
//! system opener. `docker::provision` only knows [`LoginLink`].

use kashkeshet::docker::LoginLink;
use std::io::{self, IsTerminal};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// At a terminal, shows the link and offers to open it when Enter is pressed.
/// Away from a terminal, such as in a script or for an agent, it only prints
/// the link. It never opens a browser unless asked.
pub struct TerminalLink {
    opener: String,
    pressed: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
}

impl TerminalLink {
    pub fn new() -> TerminalLink {
        TerminalLink::with_opener(system_opener())
    }

    /// A link that opens with `opener`, a program that takes the link as its
    /// only argument.
    fn with_opener(opener: impl Into<String>) -> TerminalLink {
        TerminalLink {
            opener: opener.into(),
            pressed: Arc::new(AtomicBool::new(false)),
            failure: Arc::new(Mutex::new(None)),
        }
    }
}

impl LoginLink for TerminalLink {
    fn show(&self, url: &str) {
        if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
            println!("Log in to Tailscale by opening: {url}");
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
        let mut child = std::process::Command::new(&self.opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // The opener is waited for on its own thread, which also reaps it:
        // setup does not wait, and learns later if it failed.
        let failure = Arc::clone(&self.failure);
        std::thread::spawn(move || {
            let why = match child.wait() {
                Ok(status) if status.success() => return,
                Ok(status) => format!("the opener ended with {status}"),
                Err(e) => format!("could not wait for the opener: {e}"),
            };
            if let Ok(mut slot) = failure.lock() {
                *slot = Some(why);
            }
        });
        Ok(())
    }

    fn open_failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// The program that opens a link in the person's browser.
fn system_opener() -> &'static str {
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
        assert_eq!(system_opener(), expected);
    }

    #[test]
    fn there_is_no_request_until_a_key_is_pressed_and_a_request_is_taken_once() {
        let link = TerminalLink::new();
        assert!(!link.open_requested());
        link.pressed.store(true, Ordering::SeqCst);
        assert!(link.open_requested());
        assert!(!link.open_requested());
    }

    #[test]
    fn a_missing_opener_fails_to_start() {
        let link = TerminalLink::with_opener("/no/such/kkt-opener");
        assert!(link.open("https://example.com/a").is_err());
        assert_eq!(link.open_failure(), None);
    }

    /// Waits for `link` to report a failure, for up to a few seconds.
    #[cfg(unix)]
    fn failure_of(link: &TerminalLink) -> Option<String> {
        for _ in 0..500 {
            if let Some(why) = link.open_failure() {
                return Some(why);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    // These tests run programs that already exist, never a script a test
    // writes just before running it: running a just-written file can fail
    // with "Text file busy" while other threads of the test program start
    // processes, which made them fail now and then.

    #[cfg(unix)]
    #[test]
    fn an_opener_that_exists_but_fails_is_reported_without_waiting_for_it() {
        // `false` ignores the link and exits with status 1.
        let link = TerminalLink::with_opener("false");
        assert!(link.open("https://example.com/a").is_ok());
        let why = failure_of(&link).expect("the failure is reported");
        assert!(why.contains("exit status: 1"), "{why}");
        // Reported once.
        assert_eq!(link.open_failure(), None);
    }

    #[cfg(unix)]
    #[test]
    fn an_opener_that_works_gets_the_link_and_reports_no_failure() {
        let opener = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/opener-writes-its-argument-to-a-file.sh"
        );
        // The fixture treats the link as the path of a file to write to.
        let marker = std::env::temp_dir().join(format!("kkt-opened-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let link = TerminalLink::with_opener(opener);
        assert!(link.open(marker.to_str().unwrap()).is_ok());
        // The shell makes the file, empty, a moment before it writes to it:
        // wait for what is written, not for the file.
        let written = || std::fs::read_to_string(&marker).unwrap_or_default();
        for _ in 0..500 {
            if !written().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(written().trim(), "opened");
        assert_eq!(link.open_failure(), None);
        let _ = std::fs::remove_file(&marker);
    }
}
