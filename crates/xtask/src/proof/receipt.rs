//! The receipt a run leaves: receipt.json, and receipt.md for people.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

use super::candidate::Candidate;
use super::host::{Browser, Host, Manifest};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Receipt {
    pub schema: &'static str,
    pub scope: &'static str,
    pub scope_note: &'static str,
    pub invocation: Invocation,
    pub started_at: String,
    pub candidate: Candidate,
    pub sequence_source: Source,
    pub host: Host,
    pub toolchain: Toolchain,
    pub browser_requested: &'static str,
    pub steps: Vec<Step>,
    pub finished_at: String,
    pub duration_s: f64,
    pub result: Outcome,
    pub failed_step: Option<usize>,
    pub candidate_unchanged_during_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_after: Option<Candidate>,
    pub browser: Option<Browser>,
    pub artifacts: Artifacts,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Invocation {
    pub command: String,
    pub cwd: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Source {
    pub path: &'static str,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Toolchain {
    pub rustc: String,
    pub node: String,
    pub npm: String,
}

/// How a run ended. Only a run whose every step ran and exited 0 passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Passed,
    Failed,
    Interrupted,
    Error,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed => "failed",
            Outcome::Interrupted => "interrupted",
            Outcome::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    NotRun,
    Passed,
    Failed,
    Interrupted,
    Error,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::NotRun => "not-run",
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Interrupted => "interrupted",
            Status::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Step {
    pub index: usize,
    pub command: String,
    pub cwd: &'static str,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_sha256: Option<String>,
    /// Absent for a step that never started; null for one that ended without
    /// an exit status of its own (interrupted, or an error starting it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<Option<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e2e: Option<Suite>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// The browser suite's own account of a run: its report's counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Suite {
    pub report: String,
    pub sha256: String,
    pub expected: u64,
    pub unexpected: u64,
    pub flaky: u64,
    pub skipped: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Artifacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<Option<Binary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_build_manifest: Option<Option<Manifest>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Binary {
    pub path: &'static str,
    pub sha256: String,
}

pub fn write(receipt: &Receipt, out_dir: &Path) -> io::Result<()> {
    let json = serde_json::to_string_pretty(receipt).map_err(io::Error::other)?;
    fs::write(out_dir.join("receipt.json"), json + "\n")?;
    fs::write(out_dir.join("receipt.md"), markdown(receipt))
}

/// A duration as the receipt writes seconds.
fn seconds(s: Option<f64>) -> String {
    s.map_or_else(|| "—".to_owned(), |s| format!("{s:?}"))
}

#[must_use]
pub fn markdown(r: &Receipt) -> String {
    let c = &r.candidate;
    let rows: Vec<String> = r
        .steps
        .iter()
        .map(|s| {
            let exit = match s.exit_status {
                Some(Some(code)) => code.to_string(),
                _ => "—".to_owned(),
            };
            let log = match (&s.log, &s.log_sha256) {
                (Some(log), Some(hash)) => format!("{log} `{}`", &hash[..16]),
                _ => "no log".to_owned(),
            };
            format!(
                "| {} | `{}` | {} | {exit} | {} | {log} |",
                s.index,
                s.command,
                s.status.as_str(),
                seconds(s.duration_s)
            )
        })
        .collect();
    let (binary, manifest) = (
        r.artifacts.binary.clone().flatten(),
        r.artifacts.web_build_manifest.clone().flatten(),
    );
    let suite = r
        .steps
        .iter()
        .find(|s| s.e2e.is_some() || s.problem.is_some());
    let suite_line = suite.map_or_else(
        || "not run".to_owned(),
        |s| {
            let counts = s.e2e.as_ref().map_or_else(
                || "no report".to_owned(),
                |e| {
                    format!(
                        "{} expected, {} unexpected, {} flaky, {} skipped; report sha256 `{}`",
                        e.expected, e.unexpected, e.flaky, e.skipped, e.sha256
                    )
                },
            );
            match &s.problem {
                Some(p) => format!("{counts} — {p}"),
                None => counts,
            }
        },
    );
    let b = r.browser.as_ref();
    format!(
        "### Local proof receipt — {result}

- Candidate: `{head}` tree `{tree}`{dirty}; unchanged during run: {unchanged}
- Scope: {scope} ({note})
- Command: `{command}` in `{cwd}`
- Started {started}, finished {finished}, {duration:.1}s
- Sequence source: CONTRIBUTING.md sha256 `{source}`
- Browser: {kind} `{bpath}` ({bversion})
- Host: {platform}; euid {euid}; can bypass file permissions: {privileged} (from {source_of})
- Toolchain: {rustc}; node {node}; npm {npm}
- Binary: {binary}
- Web build manifest: {manifest}
- Browser suite: {suite_line}

| # | Command | Status | Exit | Seconds | Log sha256 (prefix) |
| --- | --- | --- | --- | --- | --- |
{rows}
",
        result = r.result.as_str(),
        head = c.head,
        tree = c.tree,
        dirty = if c.dirty { " (dirty)" } else { "" },
        unchanged = r.candidate_unchanged_during_run,
        scope = r.scope,
        note = r.scope_note,
        command = r.invocation.command,
        cwd = r.invocation.cwd,
        started = r.started_at,
        finished = r.finished_at,
        duration = r.duration_s,
        source = r.sequence_source.sha256.as_deref().unwrap_or("none"),
        kind = b.map_or("not recorded", |b| b.kind),
        bpath = b.map_or("", |b| b.path.as_str()),
        bversion = b.map_or("", |b| b.version.as_str()),
        platform = r.host.platform,
        euid = r.host.euid,
        privileged = r.host.privileged,
        source_of = r.host.privilege_source,
        rustc = r.toolchain.rustc,
        node = r.toolchain.node,
        npm = r.toolchain.npm,
        binary = binary.map_or_else(
            || "not built by this run".to_owned(),
            |b| format!("`{}` sha256 `{}`", b.path, b.sha256)
        ),
        manifest = manifest.map_or_else(
            || "not built by this run".to_owned(),
            |m| format!("{} files, sha256 `{}`", m.files, m.sha256)
        ),
        rows = rows.join("\n"),
    )
}
