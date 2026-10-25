//! A signal that the server is stopping.
//!
//! Tokio-util's cancellation token does the work, but stays inside this
//! module; the MCP library is the only other part that sees it.

use tokio_util::sync::CancellationToken;

/// Clones share one signal: stopping any clone stops them all.
#[derive(Clone, Debug)]
pub struct Shutdown {
    token: CancellationToken,
}

impl Shutdown {
    pub fn new() -> Shutdown {
        Shutdown {
            token: CancellationToken::new(),
        }
    }

    /// A signal that stops when this one does, and can also be stopped on
    /// its own without stopping this one.
    pub fn child(&self) -> Shutdown {
        Shutdown {
            token: self.token.child_token(),
        }
    }

    /// Tells everything waiting on this signal to stop.
    pub fn stop(&self) {
        self.token.cancel();
    }

    pub fn is_stopped(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Finishes once the signal is stopped.
    pub async fn stopped(&self) {
        self.token.cancelled().await
    }

    /// The same signal in the form the MCP library takes.
    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Shutdown::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stopping_a_clone_stops_the_original() {
        let shutdown = Shutdown::new();
        let clone = shutdown.clone();
        assert!(!shutdown.is_stopped());
        clone.stop();
        assert!(shutdown.is_stopped());
        shutdown.stopped().await;
    }

    #[tokio::test]
    async fn a_child_stops_with_its_parent_but_not_the_reverse() {
        let parent = Shutdown::new();
        let child = parent.child();
        let other = parent.child();
        other.stop();
        assert!(!parent.is_stopped());
        assert!(!child.is_stopped());
        parent.stop();
        child.stopped().await;
        assert!(child.is_stopped());
    }

    #[test]
    fn the_library_token_is_the_same_signal() {
        let shutdown = Shutdown::new();
        let token = shutdown.cancellation_token();
        shutdown.stop();
        assert!(token.is_cancelled());
    }
}
