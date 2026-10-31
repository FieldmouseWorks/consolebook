//! Running the steps: each in its own process group, to the first failure,
//! with the receipt written however the run ends.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use super::candidate::{self, Candidate};
use super::host::{self, now, sha256_file, tool_version};
use super::lex::runs_e2e;
use super::receipt::{
    self, Artifacts, Binary, Invocation, Outcome, Receipt, Source, Status, Step, Suite, Toolchain,
};
use super::{
    BINARY, BINARY_PRODUCER, BROWSER_VAR, E2E_MARKER, ME, REPORT_NAME, REPORT_VAR, SCHEMA,
    WEB_BUILD_PRODUCER,
};

/// How long a stopped step's group gets to exit on SIGTERM before SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(10);

pub struct Options {
    /// The system Chromium the browser suite runs with; Playwright's own if
    /// none.
    pub chromium: Option<String>,
    pub invocation: Invocation,
    /// Whether the receipt records the browser and hashes the artifacts.
    pub artifacts: bool,
    /// How long a stopped step's group gets to exit on SIGTERM.
    pub grace: Duration,
    /// The shell each step runs in, as `SHELL -c STEP`.
    pub shell: PathBuf,
}

impl Options {
    #[must_use]
    pub fn new(chromium: Option<String>, invocation: Invocation) -> Options {
        Options {
            chromium,
            invocation,
            artifacts: true,
            grace: STOP_GRACE,
            shell: PathBuf::from("sh"),
        }
    }
}

/// A run, its receipt written.
#[derive(Debug)]
pub struct Ran {
    pub receipt: Receipt,
    pub dir: PathBuf,
    /// What went wrong, for a run that ended in an error.
    pub error: Option<io::Error>,
    /// A signal that came after the run ended, or after the one that ended
    /// it: nothing cuts the receipt short, so it's held until the receipt is
    /// written, for the caller to deliver.
    pub held: Option<i32>,
}

#[derive(Debug)]
pub enum RunError {
    /// Nothing ran: the candidate couldn't be read, or there's nowhere to
    /// put the receipt.
    Setup(String),
    /// The run ended, but its receipt couldn't be written there.
    Receipt(io::Error, PathBuf),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Setup(why) => f.write_str(why),
            RunError::Receipt(e, dir) => {
                write!(f, "couldn't write the receipt in {}: {e}", dir.display())
            }
        }
    }
}

impl std::error::Error for RunError {}

/// A fresh directory under `base` for one run's receipt, named for when and
/// what it ran on.
pub fn new_receipt_dir(base: &Path, head: &str, dirty: bool) -> io::Result<PathBuf> {
    let stamp = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ");
    let short: String = head.chars().take(12).collect();
    let name = format!("{stamp}-{short}{}", if dirty { "-dirty" } else { "" });
    fs::create_dir_all(base)?;
    for n in 0..100 {
        let d = base.join(if n == 0 {
            name.clone()
        } else {
            format!("{name}-{n}")
        });
        match fs::create_dir(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "could not create a fresh receipt directory under {}",
        base.display()
    )))
}

/// SIGINT and SIGTERM, as they come, while a run is under way. Each is
/// held from when it comes until it's taken, so none is lost however busy
/// the runner is when it comes; once this is dropped, they're ignored.
struct Interrupts(Signals);

impl Interrupts {
    fn start() -> io::Result<Interrupts> {
        Ok(Interrupts(Signals::new([SIGINT, SIGTERM])?))
    }

    /// One that has come and not been taken, if any.
    fn take(&mut self) -> Option<i32> {
        self.0.pending().next()
    }
}

/// How a step ended.
enum Ended {
    Exited(i32),
    Interrupted,
}

/// A step's output, to the terminal and its log, until the step's pipe
/// closes or the log is taken away.
fn tee(mut from: io::PipeReader, log: Arc<Mutex<Option<File>>>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = vec![0; 1 << 16];
        loop {
            let n = match from.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            let mut log = log.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(file) = log.as_mut() else { return };
            let mut out = io::stdout().lock();
            let _ = out.write_all(&buf[..n]);
            let _ = out.flush();
            let _ = file.write_all(&buf[..n]);
        }
    })
}

/// One step, `SHELL -c CMD` from the root in a process group of its own,
/// stdout and stderr both to the terminal and its log.
fn step(
    cmd: &str,
    root: &Path,
    env: &BTreeMap<&str, String>,
    log_path: &Path,
    opts: &Options,
    interrupts: &mut Interrupts,
) -> io::Result<Ended> {
    let log = Arc::new(Mutex::new(Some(File::create(log_path)?)));
    let (reader, writer) = io::pipe()?;
    let mut command = Command::new(&opts.shell);
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(root)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .process_group(0);
    let mut child = command.spawn()?;
    // Its ends of the pipe closed, so the step's output ends with the step.
    drop(command);
    let tee = tee(reader, log.clone());
    loop {
        if interrupts.take().is_some() {
            // The log ends here, whatever the step says while it's stopped.
            log.lock().unwrap_or_else(PoisonError::into_inner).take();
            stop_group(&mut child, opts.grace);
            return Ok(Ended::Interrupted);
        }
        // Its output ends when the step and all it started are done with it.
        if tee.is_finished() {
            let status = child.wait()?;
            // A step killed by a signal exits minus that signal's number.
            let code = status
                .code()
                .or_else(|| status.signal().map(|s| -s))
                .unwrap_or(-1);
            return Ok(Ended::Exited(code));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Ends a step and everything it started before the receipt is written.
///
/// Each step leads its own process group, so subshells, `npx` and test
/// servers are signalled with it: SIGTERM first, then SIGKILL for whatever
/// is left once the step has exited or its grace is up, so nothing in the
/// group can still change the candidate after the runner reads it again.
/// No later signal cuts this short: they wait in line.
fn stop_group(child: &mut Child, grace: Duration) {
    let group = Pid::from_child(child);
    let _ = kill_process_group(group, Signal::TERM);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
        thread::sleep(Duration::from_millis(20));
    }
    let _ = kill_process_group(group, Signal::KILL);
    let _ = child.wait();
}

/// The browser suite's own account of a run, and the problem with it, if
/// any.
fn e2e_evidence(report: &Path) -> (Option<Suite>, Option<String>) {
    if !report.is_file() {
        return (
            None,
            Some(format!(
                "the browser suite left no report ({REPORT_VAR}); it did not run"
            )),
        );
    }
    let read = fs::read(report)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|e| e.to_string()))
        .and_then(|v| {
            let count = |k: &str| {
                v["stats"][k]
                    .as_u64()
                    .ok_or_else(|| format!("its stats have no count '{k}'"))
            };
            Ok((
                count("expected")?,
                count("unexpected")?,
                count("flaky")?,
                count("skipped")?,
            ))
        })
        .and_then(|counts| Ok((counts, sha256_file(report).map_err(|e| e.to_string())?)));
    let ((expected, unexpected, flaky, skipped), sha256) = match read {
        Ok(got) => got,
        Err(e) => {
            return (
                None,
                Some(format!("the browser suite's report is unreadable: {e}")),
            );
        }
    };
    let suite = Suite {
        report: REPORT_NAME.to_owned(),
        sha256,
        expected,
        unexpected,
        flaky,
        skipped,
    };
    let problem = if expected < 1 {
        Some("the browser suite ran no tests".to_owned())
    } else if unexpected > 0 {
        Some(format!(
            "the browser suite reported {unexpected} unexpected results"
        ))
    } else {
        None
    };
    (Some(suite), problem)
}

fn seconds(since: Instant) -> f64 {
    (since.elapsed().as_secs_f64() * 1000.0).round() / 1000.0
}

/// Whether a step containing `producer` passed in this run.
#[must_use]
pub fn produced(steps: &[Step], producer: &str) -> bool {
    steps
        .iter()
        .any(|s| s.command.contains(producer) && s.status == Status::Passed)
}

fn artifacts(root: &Path, out_dir: &Path, steps: &[Step]) -> io::Result<Artifacts> {
    let binary = root.join(BINARY);
    Ok(Artifacts {
        binary: Some(if produced(steps, BINARY_PRODUCER) && binary.is_file() {
            Some(Binary {
                path: BINARY,
                sha256: sha256_file(&binary)?,
            })
        } else {
            None
        }),
        web_build_manifest: Some(if produced(steps, WEB_BUILD_PRODUCER) {
            host::web_manifest(root, out_dir)?
        } else {
            None
        }),
    })
}

/// Runs `steps` from `root` in order, stopping at the first failure, and
/// writes the receipt in a new directory under `receipts` however the run
/// ends: passed, failed, interrupted (SIGINT or SIGTERM), or an error.
pub fn run(
    steps: &[String],
    root: &Path,
    receipts: &Path,
    opts: &Options,
) -> Result<Ran, RunError> {
    let setup = |e: io::Error| RunError::Setup(e.to_string());
    let before = candidate::state(root).map_err(setup)?;
    let out_dir = new_receipt_dir(receipts, &before.head, before.dirty).map_err(setup)?;
    let report = out_dir.join(REPORT_NAME);
    let mut env = BTreeMap::new();
    if let Some(chromium) = &opts.chromium {
        env.insert(BROWSER_VAR, chromium.clone());
    }
    env.insert(REPORT_VAR, report.to_string_lossy().into_owned());
    let contributing = root.join("CONTRIBUTING.md");
    let host = host::host();
    if host.privileged {
        eprintln!(
            "{ME}: warning: this process can bypass file permissions; CONTRIBUTING.md asks for an \
             unprivileged user so permission tests are meaningful"
        );
    }
    let mut r = Receipt {
        schema: SCHEMA,
        scope: "local",
        scope_note: "local run; hosted pr-gate results are separate evidence, linked not copied",
        invocation: opts.invocation.clone(),
        started_at: now(),
        candidate: before.clone(),
        sequence_source: Source {
            path: "CONTRIBUTING.md",
            sha256: sha256_file(&contributing).ok(),
        },
        host,
        toolchain: Toolchain {
            rustc: tool_version(&["rustc", "--version"], root),
            node: tool_version(&["node", "--version"], root),
            npm: tool_version(&["npm", "--version"], root),
        },
        browser_requested: if opts.chromium.is_some() {
            "system"
        } else {
            "playwright-managed"
        },
        steps: steps
            .iter()
            .enumerate()
            .map(|(i, s)| Step {
                index: i + 1,
                command: s.clone(),
                cwd: ".",
                status: Status::NotRun,
                started_at: None,
                duration_s: None,
                log: None,
                log_sha256: None,
                exit_status: None,
                e2e: None,
                problem: None,
            })
            .collect(),
        finished_at: String::new(),
        duration_s: 0.0,
        result: Outcome::Error,
        failed_step: None,
        candidate_unchanged_during_run: true,
        candidate_after: None,
        browser: None,
        artifacts: Artifacts::default(),
    };
    match &opts.chromium {
        Some(c) => println!("{ME}: browser for '{E2E_MARKER}': system Chromium at {c}"),
        None => println!("{ME}: browser for '{E2E_MARKER}': Playwright-managed Chromium"),
    }
    println!("{ME}: receipt directory: {}", out_dir.display());

    let mut interrupts = Interrupts::start().map_err(setup)?;
    let t0 = Instant::now();
    let (result, failed, error) = steps_in_turn(
        &mut r.steps,
        root,
        &env,
        &out_dir,
        &report,
        opts,
        &mut interrupts,
    );
    if result == Outcome::Interrupted {
        eprintln!("\n{ME}: interrupted; writing the receipt");
    }
    r.duration_s = seconds(t0);
    r.result = result;
    r.failed_step = failed;
    finish(&mut r, root, &out_dir, &before, opts)
        .map_err(|e| RunError::Receipt(e, out_dir.clone()))?;
    // What came while the receipt was written, now that it's there.
    let held = interrupts.take();
    Ok(Ran {
        receipt: r,
        dir: out_dir,
        error,
        held,
    })
}

/// The steps, one after another until one doesn't pass: how the run ended,
/// the step it ended at if that one didn't pass, and the error, for one that
/// couldn't be run.
fn steps_in_turn(
    steps: &mut [Step],
    root: &Path,
    env: &BTreeMap<&str, String>,
    out_dir: &Path,
    report: &Path,
    opts: &Options,
    interrupts: &mut Interrupts,
) -> (Outcome, Option<usize>, Option<io::Error>) {
    let n = steps.len();
    for s in steps.iter_mut() {
        if interrupts.take().is_some() {
            return (Outcome::Interrupted, None, None);
        }
        let i = s.index;
        println!("\n==> [{i}/{n}] {}", s.command);
        let log = out_dir.join(format!("{i:02}.log"));
        s.started_at = Some(now());
        let s0 = Instant::now();
        let ended = step(&s.command, root, env, &log, opts, interrupts);
        s.duration_s = Some(seconds(s0));
        if log.is_file() {
            s.log = Some(format!("{i:02}.log"));
            s.log_sha256 = sha256_file(&log).ok();
        }
        let code = match ended {
            Ok(Ended::Exited(code)) => code,
            Ok(Ended::Interrupted) => {
                s.status = Status::Interrupted;
                s.exit_status = Some(None);
                return (Outcome::Interrupted, Some(i), None);
            }
            Err(e) => {
                s.status = Status::Error;
                s.exit_status = Some(None);
                eprintln!("{ME}: step {i} couldn't run: {e}");
                return (Outcome::Error, Some(i), Some(e));
            }
        };
        s.exit_status = Some(Some(code));
        s.status = if code == 0 {
            Status::Passed
        } else {
            Status::Failed
        };
        if runs_e2e(&s.command) {
            let (suite, problem) = e2e_evidence(report);
            s.e2e = suite;
            if let Some(p) = problem {
                if code == 0 {
                    s.status = Status::Failed;
                    eprintln!("{ME}: {p}");
                }
                s.problem = Some(p);
            }
        }
        println!(
            "<== [{i}/{n}] exit {code} in {:.1}s",
            s.duration_s.unwrap_or_default()
        );
        if s.status != Status::Passed {
            return (Outcome::Failed, Some(i), None);
        }
    }
    (Outcome::Passed, None, None)
}

/// The receipt's last word: the candidate read again, the browser and the
/// artifacts, then both files written.
fn finish(
    r: &mut Receipt,
    root: &Path,
    out_dir: &Path,
    before: &Candidate,
    opts: &Options,
) -> io::Result<()> {
    r.finished_at = now();
    let after = candidate::state(root)?;
    r.candidate_unchanged_during_run = after == *before;
    if after != *before {
        r.candidate_after = Some(after);
    }
    if opts.artifacts {
        r.browser = Some(host::browser(root, opts.chromium.as_deref()));
        r.artifacts = artifacts(root, out_dir, &r.steps)?;
    }
    receipt::write(r, out_dir)
}
