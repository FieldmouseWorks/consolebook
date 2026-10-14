//! Build-time inputs for the server binary.
//!
//! 1. Ensures the embedded-asset folder exists so `cargo build` succeeds
//!    before (or without) a web build. An executable built that way serves an
//!    honest "interface not embedded" notice instead of failing to compile;
//!    release packaging always builds `web/` first (see `.github/workflows/`).
//! 2. Records the source commit the binary is built from (ADR 0022). A
//!    release build sets `CONSOLEBOOK_BUILD_COMMIT` to the tagged commit, so
//!    the identity does not depend on a `.git` directory being present; any
//!    other build asks git for `HEAD`, and without git the commit is
//!    `unknown`.

use std::process::Command;

const COMMIT_VAR: &str = "CONSOLEBOOK_BUILD_COMMIT";

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let web_build = std::path::Path::new(&manifest_dir).join("../../web/build");
    std::fs::create_dir_all(&web_build).expect("creating web/build placeholder directory");
    // Re-run when the built assets change so the embed stays current.
    println!("cargo::rerun-if-changed={}", web_build.display());

    println!("cargo::rerun-if-env-changed={COMMIT_VAR}");
    let commit = std::env::var(COMMIT_VAR)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(git_head)
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo::rustc-env={COMMIT_VAR}={commit}");
}

/// The abbreviated `HEAD` commit, registering the files that move with it so
/// a new commit rebuilds the identity.
fn git_head() -> Option<String> {
    // `--git-path` resolves linked worktrees and shared refs correctly.
    for path in ["HEAD", "refs/heads", "packed-refs"] {
        if let Some(resolved) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo::rerun-if-changed={resolved}");
        }
    }
    git(&["rev-parse", "--short=12", "HEAD"])
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}
