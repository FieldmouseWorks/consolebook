//! Running the steps: fail-fast, receipts kept, the suite's own evidence,
//! errors, and interruption.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use signal_hook::consts::{SIGINT, SIGTERM};
use xtask::proof::host::sha256_file;
use xtask::proof::receipt::{Outcome, Status, Step};
use xtask::proof::run::produced;
use xtask::proof::{BINARY_PRODUCER, E2E_MARKER, REPORT_VAR};

use crate::{git, options, receipts, repo, run, saved, statuses};

#[test]
fn a_failing_step_stops_the_chain_and_keeps_its_receipt() {
    let (root, (_k, base)) = (repo(), receipts());
    let opts = options();
    let failed = run(
        &["echo one", "echo two >&2; exit 3", "echo three"],
        root.path(),
        &base,
        &opts,
    );
    let r = saved(&failed.dir);
    assert_eq!(failed.receipt.result, Outcome::Failed);
    assert_eq!(failed.receipt.failed_step, Some(2));
    assert_eq!(
        (r["result"].as_str(), r["failed_step"].as_u64()),
        (Some("failed"), Some(2))
    );
    assert_eq!(statuses(&r), ["passed", "failed", "not-run"]);
    assert_eq!(r["steps"][1]["exit_status"], 3);
    assert!(r["steps"][2].get("exit_status").is_none(), "{r}");
    assert!(!failed.dir.join("03.log").exists(), "step 3 started");
    assert_eq!(fs::read(failed.dir.join("02.log")).unwrap(), b"two\n");
    assert_eq!(
        r["steps"][1]["log_sha256"].as_str().unwrap(),
        sha256_file(&failed.dir.join("02.log")).unwrap()
    );
    assert_eq!(r["schema"], "consolebook-proof-receipt/1");
    assert_eq!(r["candidate_unchanged_during_run"], true);
    let md = fs::read_to_string(failed.dir.join("receipt.md")).unwrap();
    assert!(md.starts_with("### Local proof receipt — failed"), "{md}");

    let passed = run(&["true"], root.path(), &base, &opts);
    assert_eq!(passed.receipt.result, Outcome::Passed);
    assert_eq!(passed.receipt.failed_step, None);
    assert_ne!(passed.dir, failed.dir);
    assert!(
        failed.dir.join("receipt.json").is_file(),
        "the failed receipt is gone"
    );
    assert!(passed.error.is_none() && passed.held.is_none());
}

#[test]
fn a_candidate_that_changes_meanwhile_is_recorded() {
    let (root, (_k, base)) = (repo(), receipts());
    let ran = run(&["echo x > new"], root.path(), &base, &options());
    let r = saved(&ran.dir);
    assert_eq!(r["candidate_unchanged_during_run"], false);
    assert!(
        r["candidate_after"]["untracked_sha256"]["new"].is_string(),
        "{r}"
    );
    assert_eq!(r["result"], "passed");
}

/// The browser step passes only on the suite's own report of passing tests.
#[test]
fn a_passing_exit_is_not_enough_without_the_suites_report() {
    let (root, (_k, base)) = (repo(), receipts());
    let opts = options();
    let unreached = format!("exit 0 && {E2E_MARKER}");
    let ran = run(&[&unreached, "echo never"], root.path(), &base, &opts);
    let step = &ran.receipt.steps[0];
    assert_eq!(
        (ran.receipt.result, ran.receipt.failed_step),
        (Outcome::Failed, Some(1))
    );
    assert!(
        step.problem
            .as_deref()
            .unwrap_or("")
            .contains("did not run"),
        "{step:?}"
    );
    assert_eq!(ran.receipt.steps[1].status, Status::NotRun);
    let md = fs::read_to_string(ran.dir.join("receipt.md")).unwrap();
    assert!(md.contains("did not run"), "{md}");

    let fake = |stats: &str| {
        format!("printf '{{\"stats\":{stats}}}' > \"${REPORT_VAR}\" && exit 0 && {E2E_MARKER}")
    };
    let none = fake(r#"{"expected":0,"unexpected":0,"flaky":0,"skipped":4}"#);
    let ran = run(&[&none], root.path(), &base, &opts);
    assert_eq!(ran.receipt.result, Outcome::Failed);
    assert!(
        ran.receipt.steps[0]
            .problem
            .as_deref()
            .unwrap_or("")
            .contains("no tests")
    );
    let bad = fake(r#"{"expected":3,"unexpected":1,"flaky":0,"skipped":0}"#);
    let ran = run(&[&bad], root.path(), &base, &opts);
    assert_eq!(ran.receipt.result, Outcome::Failed);
    assert!(
        ran.receipt.steps[0]
            .problem
            .as_deref()
            .unwrap_or("")
            .contains("1 unexpected")
    );
    let garbled = fake(r#"{"expected":"many"}"#);
    let ran = run(&[&garbled], root.path(), &base, &opts);
    assert!(
        ran.receipt.steps[0]
            .problem
            .as_deref()
            .unwrap_or("")
            .contains("unreadable")
    );

    let good = fake(r#"{"expected":3,"unexpected":0,"flaky":0,"skipped":0}"#);
    let ran = run(&[&good], root.path(), &base, &opts);
    assert_eq!(ran.receipt.result, Outcome::Passed);
    let suite = ran.receipt.steps[0].e2e.as_ref().unwrap();
    assert_eq!(suite.expected, 3);
    assert_eq!(
        suite.sha256,
        sha256_file(&ran.dir.join("e2e-report.json")).unwrap()
    );
}

/// A step that can't be run at all is an error, never a pass.
#[test]
fn an_error_running_a_step_is_recorded_as_one() {
    let (root, (_k, base)) = (repo(), receipts());
    let mut opts = options();
    opts.shell = PathBuf::from("/nonexistent/sh");
    let ran = run(&["true", "true"], root.path(), &base, &opts);
    assert!(ran.error.is_some());
    let r = saved(&ran.dir);
    assert_eq!(
        (r["result"].as_str(), r["failed_step"].as_u64()),
        (Some("error"), Some(1))
    );
    assert_eq!(statuses(&r), ["error", "not-run"]);
    assert!(r["steps"][0]["exit_status"].is_null(), "{r}");
}

#[test]
fn sigterm_mid_run_keeps_an_interrupted_receipt() {
    let (root, (_k, base)) = (repo(), receipts());
    let ran = run(
        &["kill -TERM $PPID; sleep 5", "echo never"],
        root.path(),
        &base,
        &options(),
    );
    let r = saved(&ran.dir);
    assert_eq!(
        (r["result"].as_str(), r["failed_step"].as_u64()),
        (Some("interrupted"), Some(1))
    );
    assert_eq!(statuses(&r), ["interrupted", "not-run"]);
    assert!(r["steps"][0]["exit_status"].is_null(), "{r}");
    assert!(r["steps"][0]["duration_s"].as_f64().unwrap() < 4.0, "{r}");
    assert_eq!(ran.held, None);
}

/// Nothing a stopped step started can change the candidate after the
/// runner reads it again: a child that ignores SIGTERM is killed with it.
#[test]
fn a_child_that_ignores_sigterm_is_stopped() {
    let (root, (k, base)) = (repo(), receipts());
    let marker = k.path().join("late-write");
    let step = format!(
        "(trap '' TERM; sleep 2; touch '{}') & kill -TERM $PPID; wait",
        marker.display()
    );
    let ran = run(&[&step], root.path(), &base, &options());
    assert_eq!(ran.receipt.result, Outcome::Interrupted);
    thread::sleep(Duration::from_secs(3));
    assert!(!marker.exists(), "it wrote after it was stopped");
}

/// What a step says once it's been stopped isn't in its log, so the log the
/// receipt hashes is the one it keeps.
#[test]
fn a_stopped_steps_log_ends_at_the_signal() {
    let (root, (_k, base)) = (repo(), receipts());
    let mut opts = options();
    opts.grace = Duration::from_secs(1);
    let ran = run(
        &["trap '' TERM; echo before; sleep 0.3; kill -TERM $PPID; sleep 0.3; echo after; sleep 5"],
        root.path(),
        &base,
        &opts,
    );
    let r = saved(&ran.dir);
    let log = ran.dir.join("01.log");
    assert_eq!(fs::read_to_string(&log).unwrap(), "before\n");
    assert_eq!(r["steps"][0]["log_sha256"], sha256_file(&log).unwrap());
}

/// A signal while the step is stopped can't skip the kill, and isn't lost:
/// it waits until the receipt is written.
#[test]
fn a_repeated_signal_neither_skips_the_kill_nor_is_lost() {
    let (root, (k, base)) = (repo(), receipts());
    let marker = k.path().join("late-write");
    let mut opts = options();
    opts.grace = Duration::from_secs(1);
    let step = format!(
        "trap '' TERM; kill -TERM $PPID; sleep 0.3; kill -TERM $PPID; sleep 2; touch '{}'",
        marker.display()
    );
    let ran = run(&[&step], root.path(), &base, &opts);
    assert_eq!(ran.receipt.result, Outcome::Interrupted);
    assert_eq!(ran.held, Some(SIGTERM));
    thread::sleep(Duration::from_millis(2500));
    assert!(!marker.exists(), "it wrote after the grace period");

    let ran = run(
        &["trap '' TERM; kill -INT $PPID; sleep 0.3; kill -INT $PPID; sleep 5"],
        root.path(),
        &base,
        &opts,
    );
    assert_eq!(saved(&ran.dir)["result"], "interrupted");
    assert_eq!(ran.held, Some(SIGINT));
}

/// A signal while the receipt is written waits for it: the receipt is kept,
/// and the signal is delivered once it's there.
#[test]
fn a_signal_while_the_receipt_is_written_waits_for_it() {
    let (root, (k, base)) = (repo(), receipts());
    let stopped = k.path().join("stopped");
    // git runs its fsmonitor hook each time the runner reads the candidate,
    // so this one signals the runner as it reads it again, after the step.
    let hook = k.path().join("fsmonitor");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\n[ -e '{0}' ] && rm '{0}' && kill -INT {1}\nexit 1\n",
            stopped.display(),
            std::process::id()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    git(
        root.path(),
        &["config", "core.fsmonitor", hook.to_str().unwrap()],
    );
    let step = format!("touch '{}'; kill -INT $PPID; sleep 5", stopped.display());
    let ran = run(&[&step], root.path(), &base, &options());
    assert!(!stopped.exists(), "the candidate wasn't read again");
    assert_eq!(saved(&ran.dir)["result"], "interrupted");
    assert_eq!(ran.held, Some(SIGINT));
}

#[test]
fn artifacts_are_hashed_only_when_their_producer_passed() {
    let step = |status| Step {
        index: 1,
        command: format!("x && {BINARY_PRODUCER}"),
        cwd: ".",
        status,
        started_at: None,
        duration_s: None,
        log: None,
        log_sha256: None,
        exit_status: None,
        e2e: None,
        problem: None,
    };
    assert!(!produced(&[step(Status::NotRun)], BINARY_PRODUCER));
    assert!(!produced(&[step(Status::Failed)], BINARY_PRODUCER));
    assert!(produced(&[step(Status::Passed)], BINARY_PRODUCER));
}
