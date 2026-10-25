//! Exercise actual OS signals against both installed binary names.
#![cfg(unix)]

#[path = "common/process.rs"]
mod process;
use process::{OpenRequest, stops_cleanly};

#[tokio::test]
async fn sigterm_stops_both_binaries_cleanly() {
    for binary in [env!("CARGO_BIN_EXE_kashkeshet"), env!("CARGO_BIN_EXE_kkt")] {
        stops_cleanly(binary, "-TERM", "SIGTERM", OpenRequest::None).await;
    }
}

#[tokio::test]
async fn ctrl_c_still_stops_cleanly() {
    stops_cleanly(
        env!("CARGO_BIN_EXE_kashkeshet"),
        "-INT",
        "Ctrl-C",
        OpenRequest::None,
    )
    .await;
}

#[tokio::test]
async fn shutdown_ends_open_event_streams() {
    for (signal, log) in [("-TERM", "SIGTERM"), ("-INT", "Ctrl-C")] {
        stops_cleanly(
            env!("CARGO_BIN_EXE_kashkeshet"),
            signal,
            log,
            OpenRequest::Events,
        )
        .await;
    }
}

#[tokio::test]
async fn shutdown_ends_pending_message_waits() {
    for (signal, log) in [("-TERM", "SIGTERM"), ("-INT", "Ctrl-C")] {
        stops_cleanly(
            env!("CARGO_BIN_EXE_kashkeshet"),
            signal,
            log,
            OpenRequest::Wait,
        )
        .await;
    }
}
