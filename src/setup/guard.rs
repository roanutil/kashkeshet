//! Whether `kkt serve` may run on this machine, given its setup record.

use super::{Mode, Record};

/// Whether `kkt serve` may start here. `record` is the setup record, if any.
/// `service_running` is whether the kkt service is running, when that could
/// be found out; it matters only for a machine set up with the service.
/// `ignore` is `--ignore-setup`, which the service itself passes.
///
/// The error says where this machine's server really is.
pub fn guard(
    record: Option<&Record>,
    service_running: Option<bool>,
    ignore: bool,
) -> Result<(), String> {
    let Some(record) = record.filter(|_| !ignore) else {
        return Ok(());
    };
    match &record.mode {
        Mode::Service { .. } if service_running == Some(true) => Err(
            "kkt already runs on this machine as a service (see `kkt status`); \
             stop it with `kkt service stop`, or pass --ignore-setup to start another server"
                .to_string(),
        ),
        Mode::Service { .. } => Ok(()),
        Mode::Docker(docker) => Err(format!(
            "this machine runs kkt in Docker, from {} (see `kkt status`); \
             pass --ignore-setup to start another server",
            docker.folder.display()
        )),
        Mode::Client => Err(format!(
            "this machine is set up as a client of the kkt server at {}; \
             run `kkt setup` to change that, or pass --ignore-setup to start a server anyway",
            record.url
        )),
    }
}
