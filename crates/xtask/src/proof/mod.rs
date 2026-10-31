//! `cargo xtask proof`: CONTRIBUTING.md's gate sequence, run in order to the
//! first failure, with a receipt left behind (#80).
//!
//! Single source: the sequence isn't written here. It's read from the first
//! ```` ```sh ```` block under "## Build and verification" in CONTRIBUTING.md;
//! each non-blank, non-comment line is one step, run with `sh -c` from the
//! repository root. With `--chromium` (or `CONSOLEBOOK_E2E_CHROMIUM` in the
//! environment), the one step running `npm run e2e` is replaced by the
//! system-browser line from the block in that section that sets
//! `CONSOLEBOOK_E2E_CHROMIUM=/path/to/chromium`. Anything else about those
//! blocks changing makes it fail closed.
//!
//! The parser can't prove a shell line reaches the suite (`exit 0 && npm run
//! e2e` reads as an invocation), so the browser step must also leave runtime
//! evidence: the runner points `CONSOLEBOOK_E2E_REPORT` at the receipt
//! directory, web/playwright.config.ts writes a JSON report there, and the
//! step passes only when that report exists and counts at least one expected
//! test and no unexpected ones.
//!
//! Threat model: the runner catches honest drift (a gate that stops reaching
//! the suite, a suite that finds no tests, a masked failure, a candidate that
//! changes mid-run). It trusts the reviewed gate definition and the
//! repository code it executes: whoever can edit CONTRIBUTING.md can also
//! edit this crate, the Playwright config or the tests, so no check here can
//! authenticate a gate written to deceive it. The receipt records
//! CONTRIBUTING.md's hash so review can see which definition ran.
//!
//! Receipts: each run writes a new directory under `.proof/` (ignored, never
//! deleted or overwritten here) holding receipt.json, receipt.md, one log per
//! step, and the web asset manifest. Failed and interrupted runs keep theirs.
//! The receipt is local evidence only; hosted pr-gate results are linked, not
//! copied.

pub mod candidate;
pub mod host;
pub mod lex;
pub mod receipt;
pub mod run;
pub mod sequence;

/// How the receipt names the command that ran it.
pub const ME: &str = "cargo xtask proof";
pub const SCHEMA: &str = "consolebook-proof-receipt/1";
pub const SECTION: &str = "## Build and verification";
pub const E2E_MARKER: &str = "npm run e2e";
pub const BROWSER_VAR: &str = "CONSOLEBOOK_E2E_CHROMIUM";
pub const REPORT_VAR: &str = "CONSOLEBOOK_E2E_REPORT";
pub const REPORT_NAME: &str = "e2e-report.json";
pub const BROWSER_PLACEHOLDER: &str = "/path/to/chromium";
pub const RECEIPT_ROOT: &str = ".proof";
pub const BINARY: &str = "target/debug/consolebook-server";
pub const WEB_BUILD: &str = "web/build";
/// An artifact is hashed only when a step containing its producer passed in
/// this run; otherwise a leftover from an earlier build would pass as
/// evidence.
pub const BINARY_PRODUCER: &str = "cargo build -p consolebook-server";
pub const WEB_BUILD_PRODUCER: &str = "npm run build";

/// A precondition that failed before any step ran (exit 2).
#[derive(Debug, PartialEq, Eq)]
pub struct SetupError(pub String);

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SetupError {}

fn setup(why: impl Into<String>) -> SetupError {
    SetupError(why.into())
}
