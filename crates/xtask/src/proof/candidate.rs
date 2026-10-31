//! The candidate a run is about: its revision, tree and dirty state.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt::Write;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

use super::host::{sha256, sha256_file};

/// `git ARGS` in `root`: what it printed.
pub fn git(root: &Path, args: &[&str]) -> io::Result<Vec<u8>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(io::Error::other(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// One entry of `git status`, its paths as they are on disk.
pub struct Entry {
    /// The two-letter code.
    pub code: String,
    pub path: Vec<u8>,
    /// A rename's or copy's second path, the one it came from.
    pub source: Option<Vec<u8>>,
}

/// `git status`, entry by entry.
///
/// Reads the NUL-separated form, so names with spaces, quotes, or non-ASCII
/// characters arrive exactly as on disk. Either column can carry a rename:
/// `X` for a staged one, `Y` for a worktree rename of an intent-to-add file.
pub fn status(root: &Path) -> io::Result<Vec<Entry>> {
    let raw = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut fields = raw.split(|b| *b == 0);
    let mut entries = Vec::new();
    while let Some(record) = fields.next() {
        if record.len() < 3 {
            continue;
        }
        let code = String::from_utf8_lossy(&record[..2]).into_owned();
        let source = if code.contains(['R', 'C']) {
            Some(fields.next().unwrap_or_default().to_vec())
        } else {
            None
        };
        entries.push(Entry {
            code,
            path: record[3..].to_vec(),
            source,
        });
    }
    Ok(entries)
}

/// A file name as text, one to one: UTF-8 as it is, but for `\` and `>`,
/// escaped as `\\` and `\>`, and each byte that isn't UTF-8 as `\xNN`.
/// So no two names read the same (a name with U+FFFD in it included), and a
/// rename's `SOURCE -> PATH` splits only one way.
#[must_use]
pub fn name(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    for chunk in raw.utf8_chunks() {
        for c in chunk.valid().chars() {
            if matches!(c, '\\' | '>') {
                out.push('\\');
            }
            out.push(c);
        }
        for b in chunk.invalid() {
            let _ = write!(out, "\\x{b:02x}");
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Candidate {
    pub head: String,
    pub tree: String,
    pub dirty: bool,
    /// `git status`, one entry a line, each name as [`name`] writes it.
    pub status: Vec<String>,
    pub tracked_diff_sha256: String,
    /// Each untracked file's hash, by its name as [`name`] writes it; null
    /// for one that isn't a regular file.
    pub untracked_sha256: BTreeMap<String, Option<String>>,
}

/// The candidate at `root` as it is now.
pub fn state(root: &Path) -> io::Result<Candidate> {
    let entries = status(root)?;
    let mut untracked = BTreeMap::new();
    for e in &entries {
        if e.code == "??" {
            let p = root.join(OsStr::from_bytes(&e.path));
            let hash = if p.is_file() {
                Some(sha256_file(&p)?)
            } else {
                None
            };
            untracked.insert(name(&e.path), hash);
        }
    }
    let rev = |what: &str| -> io::Result<String> {
        Ok(String::from_utf8_lossy(&git(root, &["rev-parse", what])?)
            .trim()
            .to_owned())
    };
    Ok(Candidate {
        head: rev("HEAD")?,
        tree: rev("HEAD^{tree}")?,
        dirty: !entries.is_empty(),
        status: entries
            .iter()
            .map(|e| match &e.source {
                Some(source) => format!("{} {} -> {}", e.code, name(source), name(&e.path)),
                None => format!("{} {}", e.code, name(&e.path)),
            })
            .collect(),
        tracked_diff_sha256: sha256(&git(root, &["diff", "HEAD", "--binary"])?),
        untracked_sha256: untracked,
    })
}
