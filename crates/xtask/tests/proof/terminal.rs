//! The terminal is only a view of a run: a reader that goes away mid-run
//! doesn't cost the receipt.

use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use xtask::proof::{BROWSER_PLACEHOLDER, BROWSER_VAR, E2E_MARKER, SECTION};

use crate::{repo, saved, statuses};

#[test]
fn a_terminal_that_goes_away_doesnt_cost_the_receipt() {
    let root = repo();
    let r = root.path();
    fs::write(r.join(".gitignore"), ".proof/\n").unwrap();
    fs::write(
        r.join("CONTRIBUTING.md"),
        format!(
            "{SECTION}\n\n```sh\nsleep 0.5; echo after\nexit 0 && {E2E_MARKER}\n```\n\n\
             ```sh\n{BROWSER_VAR}={BROWSER_PLACEHOLDER} {E2E_MARKER}\n```\n"
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("proof")
        .current_dir(r)
        .env_remove(BROWSER_VAR)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Read until the first step starts, then go away while it runs.
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while !line.starts_with("==> [1/2]") {
        line.clear();
        assert_ne!(out.read_line(&mut line).unwrap(), 0, "no first step");
    }
    drop(out);
    let status = child.wait().unwrap();
    // Failed: the browser step left no report. A panic would be 101.
    assert_eq!(status.code(), Some(1), "{status:?}");
    let dirs: Vec<_> = fs::read_dir(r.join(".proof"))
        .unwrap()
        .map(|d| d.unwrap().path())
        .collect();
    let [dir] = dirs.as_slice() else {
        panic!("{dirs:?}")
    };
    let receipt = saved(dir);
    assert_eq!(receipt["result"], "failed");
    assert_eq!(statuses(&receipt), ["passed", "failed"]);
    assert!(dir.join("receipt.md").is_file());
}
