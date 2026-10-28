//! `kkt setup`: what this machine is set up as (a server run as a service, a
//! server in Docker, or a client of a server elsewhere), the record that
//! says so, and what follows from it: whether `kkt serve` may run here.
//!
//! Like `settings/`, this part reads no files and runs nothing itself. The
//! caller reads and writes the record's text, and answers questions through
//! [`Ask`], so every rule here is tested directly.

mod ask;
mod flow;
mod guard;
mod record;
#[cfg(test)]
mod tests;
mod upgrade;

pub use ask::{Ask, Question, unanswered};
pub use flow::{Answers, Installed, Purpose, Steps, run};
pub use guard::guard;
pub use record::{DockerSetup, Mode, Record};
pub use upgrade::{Upgrade, upgrade};

use crate::config::SETUP_FILE_NAME;
use crate::settings::Env;
use std::path::PathBuf;

/// Where the setup record is: `setup.toml` in the config directory, beside
/// the config file and the access key.
pub fn record_path(env: &Env) -> Option<PathBuf> {
    Some(env.config_dir()?.join(SETUP_FILE_NAME))
}
