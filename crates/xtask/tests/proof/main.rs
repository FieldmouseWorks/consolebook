//! The proof runner's controls: each would fail on the bug it's for.

mod candidate;
mod parse;
mod runner;
mod terminal;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tempfile::TempDir;
use xtask::proof::receipt::Invocation;
use xtask::proof::run::{self, Options, Ran};

/// The runs take turns: a step's signal to the runner reaches the whole test
/// process, and with it any run beside it.
static ONE_RUN: Mutex<()> = Mutex::new(());

pub fn one_run() -> MutexGuard<'static, ()> {
    ONE_RUN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `git ARGS` in `dir`, as an invented author, nothing of the machine's own
/// config read.
pub fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args([
            "-c",
            "user.name=Invented",
            "-c",
            "user.email=invented@example.invalid",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// A repository with one empty commit: the candidate a run is about.
#[must_use]
pub fn repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["commit", "-q", "--allow-empty", "-m", "invented"],
    );
    dir
}

#[must_use]
pub fn options() -> Options {
    let mut o = Options::new(
        None,
        Invocation {
            command: "cargo xtask proof".to_owned(),
            cwd: "/invented".to_owned(),
        },
    );
    o.artifacts = false;
    o
}

/// `steps` run from `root`, their receipt under `receipts`.
#[must_use]
pub fn run(steps: &[&str], root: &Path, receipts: &Path, opts: &Options) -> Ran {
    let _turn = one_run();
    let steps: Vec<String> = steps.iter().map(|s| (*s).to_owned()).collect();
    run::run(&steps, root, receipts, opts).unwrap()
}

/// The receipt.json a run left.
#[must_use]
pub fn saved(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("receipt.json")).unwrap()).unwrap()
}

#[must_use]
pub fn statuses(receipt: &serde_json::Value) -> Vec<String> {
    receipt["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["status"].as_str().unwrap().to_owned())
        .collect()
}

/// A directory of its own for receipts, outside the candidate.
#[must_use]
pub fn receipts() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipts");
    (dir, path)
}
