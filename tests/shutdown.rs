//! Exercise actual OS signals against `kkt serve`.
#![cfg(unix)]

#[path = "common/home.rs"]
mod home;
#[path = "common/process.rs"]
mod process;
use process::{OpenRequest, stops_cleanly};

#[tokio::test]
async fn sigterm_stops_the_server_cleanly() {
    stops_cleanly("-TERM", "SIGTERM", OpenRequest::None).await;
}

#[tokio::test]
async fn ctrl_c_still_stops_cleanly() {
    stops_cleanly("-INT", "Ctrl-C", OpenRequest::None).await;
}

#[tokio::test]
async fn shutdown_ends_open_event_streams() {
    for (signal, log) in [("-TERM", "SIGTERM"), ("-INT", "Ctrl-C")] {
        stops_cleanly(signal, log, OpenRequest::Events).await;
    }
}

#[tokio::test]
async fn shutdown_ends_pending_message_waits() {
    for (signal, log) in [("-TERM", "SIGTERM"), ("-INT", "Ctrl-C")] {
        stops_cleanly(signal, log, OpenRequest::Wait).await;
    }
}
