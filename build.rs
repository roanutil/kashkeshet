//! Records the git commit the server is built from, for provenance. The
//! server reads it as the `KASHKESHET_GIT_COMMIT` environment variable at
//! compile time. Without git, it is `unknown`.

use std::path::Path;
use std::process::Command;

#[path = "src/provenance/commit.rs"]
mod commit;

fn main() {
    let head = git(&["rev-parse", "HEAD"]);
    let dirty = git(&["status", "--porcelain"]).map(|out| !out.trim().is_empty());
    let text = commit::commit_text(head.as_deref(), dirty);
    println!("cargo:rustc-env=KASHKESHET_GIT_COMMIT={text}");

    // Run again when the commit or the working tree changes.
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        let git_dir = Path::new(git_dir.trim());
        for name in ["HEAD", "index", "packed-refs"] {
            rerun_if_exists(&git_dir.join(name));
        }
        if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
            rerun_if_exists(&git_dir.join(branch.trim()));
        }
    }
    for path in [
        "build.rs",
        "Cargo.toml",
        "src",
        "migrations",
        "agent-instructions.md",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}

/// What git prints for `args`, or `None` if git is missing or fails.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn rerun_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
