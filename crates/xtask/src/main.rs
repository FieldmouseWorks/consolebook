//! The repository's own tools.
//!
//! `cargo xtask proof [--chromium PATH] [--list]`: CONTRIBUTING.md's gate
//! sequence, run in order, stopping at the first failure, with a receipt
//! left under `.proof/` (see `xtask::proof`). Exit: 0 every gate passed, 1 a
//! gate failed or the run was interrupted, 2 usage or setup error (nothing
//! was run).
//!
//! `cargo xtask drill seed|collect|compare …`: the restore drill's HTTP side
//! (see `xtask::drill`). Exit: 0 done (for `compare`, the manifests hold the
//! same), 1 the drill failed (the manifests differ), 2 usage error.

#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use xtask::drill::account::Account;
use xtask::drill::{DrillError, collect, compare, seed};
use xtask::proof::host::sha256_file;
use xtask::proof::lex::join;
use xtask::proof::receipt::{Invocation, Outcome, markdown};
use xtask::proof::run::{Options, RunError, run};
use xtask::proof::sequence::{parse, with_system_browser};
use xtask::proof::{BROWSER_VAR, ME, RECEIPT_ROOT, SetupError};
use xtask::{note, say};

const USAGE: &str = "usage: cargo xtask proof [--chromium PATH] [--list]

  cargo xtask proof                    run every gate in order, stop at the first failure
  cargo xtask proof --chromium PATH    run the browser suite against a system Chromium
                                       (default: $CONSOLEBOOK_E2E_CHROMIUM if set)
  cargo xtask proof --list             print the parsed sequence and exit";

const DRILL_USAGE: &str = "usage: cargo xtask drill seed BASE_URL SETUP_CODE ACCOUNT OUT_DIR
       cargo xtask drill collect BASE_URL ACCOUNT OUT_DIR
       cargo xtask drill compare BEFORE AFTER

  seed      set up the fresh installation at BASE_URL with invented people and
            records, write its administrator's sign-in to ACCOUNT (a new file),
            then collect into OUT_DIR
  collect   sign in with ACCOUNT and write OUT_DIR/manifest.json: every finalized
            record's version metadata and acknowledgment, and what its export and
            the installation's export declare, with the exports saved beside it
  compare   compare two manifest.json files; exit 0 if they hold the same, 1 if not";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.split_first() {
        Some((first, rest)) if first == "proof" => proof(rest),
        Some((first, rest)) if first == "drill" => drill(rest),
        _ => {
            note!("{USAGE}\n\n{DRILL_USAGE}");
            ExitCode::from(2)
        }
    }
}

fn drill(args: &[String]) -> ExitCode {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let done = match words.as_slice() {
        ["-h" | "--help"] => {
            say!("{DRILL_USAGE}");
            return ExitCode::SUCCESS;
        }
        ["seed", base, code, account, out] => {
            seed::seed(base, code, Path::new(account)).and_then(|seeded| {
                say!(
                    "seeded {} finalized records for enrollment {}",
                    seeded.records,
                    seeded.enrollment
                );
                collected(base, &seeded.admin, out)
            })
        }
        ["collect", base, account, out] => {
            Account::read(Path::new(account)).and_then(|admin| collected(base, &admin, out))
        }
        ["compare", before, after] => return compared(Path::new(before), Path::new(after)),
        _ => {
            note!("{DRILL_USAGE}");
            return ExitCode::from(2);
        }
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            note!("drill: {e}");
            ExitCode::FAILURE
        }
    }
}

fn collected(base: &str, admin: &Account, out: &str) -> Result<(), DrillError> {
    let collected = collect::collect(base, admin, Path::new(out))?;
    say!(
        "collected {} records; manifest sha256 {}",
        collected.records,
        collected.sha256
    );
    Ok(())
}

fn compared(before: &Path, after: &Path) -> ExitCode {
    let read = |path: &Path| -> Result<serde_json::Value, String> {
        let text = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", path.display()))
    };
    let seen = read(before)
        .and_then(|a| read(after).map(|b| (a, b)))
        .and_then(|(a, b)| compare::compare(&a, &b).map_err(|e| e.to_string()));
    match seen {
        Ok(seen) => {
            for line in &seen.report {
                say!("{line}");
            }
            if seen.same {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            note!("drill: {e}");
            ExitCode::from(2)
        }
    }
}

struct Args {
    chromium: Option<String>,
    list: bool,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args {
        chromium: None,
        list: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--list" => out.list = true,
            "--chromium" => {
                out.chromium = Some(it.next().ok_or("--chromium needs a PATH")?.clone());
            }
            _ => match a.strip_prefix("--chromium=") {
                Some(path) => out.chromium = Some(path.to_owned()),
                None => return Err(format!("unknown argument: {a}")),
            },
        }
    }
    Ok(out)
}

fn proof(given: &[String]) -> ExitCode {
    if given.iter().any(|a| a == "-h" || a == "--help") {
        say!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let flags = match parse_args(given) {
        Ok(a) => a,
        Err(e) => {
            note!("{ME}: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Ok(top) = xtask::proof::candidate::git(&cwd, &["rev-parse", "--show-toplevel"]) else {
        note!("{ME}: not inside a git repository");
        return ExitCode::from(2);
    };
    let root = PathBuf::from(String::from_utf8_lossy(&top).trim());
    match gate(&root, &cwd, given, flags) {
        Ok(code) => code,
        Err(e) => {
            note!("{ME}: {e}");
            ExitCode::from(2)
        }
    }
}

fn gate(root: &Path, cwd: &Path, given: &[String], flags: Args) -> Result<ExitCode, SetupError> {
    let text = std::fs::read_to_string(root.join("CONTRIBUTING.md"))
        .map_err(|e| SetupError(format!("CONTRIBUTING.md: {e}")))?;
    let (mut steps, browser_line) = parse(&text)?;
    let chromium = flags
        .chromium
        .or_else(|| env::var(BROWSER_VAR).ok())
        .filter(|c| !c.is_empty());
    let chromium = match chromium {
        Some(c) => {
            let path = Path::new(&c);
            let runnable =
                path.is_file() && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok();
            if !runnable {
                return Err(SetupError(format!(
                    "--chromium is not an executable file: {c}"
                )));
            }
            let abs = std::path::absolute(path)
                .map_err(|e| SetupError(format!("--chromium {c}: {e}")))?
                .to_string_lossy()
                .into_owned();
            steps = with_system_browser(&steps, &browser_line, &abs)?;
            Some(abs)
        }
        None => None,
    };
    if flags.list {
        for (i, s) in steps.iter().enumerate() {
            say!("{}. {s}", i + 1);
        }
        return Ok(ExitCode::SUCCESS);
    }
    let words: Vec<&str> = ME
        .split(' ')
        .chain(given.iter().map(String::as_str))
        .collect();
    let invocation = Invocation {
        command: join(&words),
        cwd: cwd.to_string_lossy().into_owned(),
    };
    let ran = match run(
        &steps,
        root,
        &root.join(RECEIPT_ROOT),
        &Options::new(chromium, invocation),
    ) {
        Ok(ran) => ran,
        Err(RunError::Setup(why)) => return Err(SetupError(why)),
        Err(e @ RunError::Receipt(..)) => {
            note!("{ME}: {e}");
            return Ok(ExitCode::FAILURE);
        }
    };
    let r = &ran.receipt;
    say!("\n{}", markdown(r));
    let json = ran.dir.join("receipt.json");
    say!(
        "{ME}: {}; receipt {} sha256 {}",
        r.result.as_str(),
        json.display(),
        sha256_file(&json).unwrap_or_default()
    );
    if !r.candidate_unchanged_during_run {
        note!("{ME}: warning: the candidate changed during the run; this receipt is stale");
    }
    if let Some(e) = &ran.error {
        note!("{ME}: the run ended in an error: {e}");
    }
    if let Some(sig) = ran.held {
        // The signal that came while the receipt was written, delivered now.
        let _ = signal_hook::low_level::emulate_default_handler(sig);
    }
    Ok(if r.result == Outcome::Passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
