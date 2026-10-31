//! The candidate a run is about: its revision, tree and dirty state.

use std::collections::BTreeMap;
use std::ffi::OsStr;
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

/// `git status` as (two-letter code, path) pairs, the path as it is on disk.
///
/// Reads the NUL-separated form, so names with spaces, quotes, or non-ASCII
/// characters arrive exactly as on disk; a rename's or copy's second (source)
/// path is folded into its entry, `SOURCE -> PATH`. Either column can carry
/// the rename: `X` for a staged one, `Y` for a worktree rename of an
/// intent-to-add file.
pub fn status(root: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
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
        let mut path = record[3..].to_vec();
        if code.contains(['R', 'C']) {
            let source = fields.next().unwrap_or_default();
            path = [source, b" -> ", &path].concat();
        }
        entries.push((code, path));
    }
    Ok(entries)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Candidate {
    pub head: String,
    pub tree: String,
    pub dirty: bool,
    pub status: Vec<String>,
    pub tracked_diff_sha256: String,
    /// Each untracked file's hash, by its name; null for one that isn't a
    /// regular file.
    pub untracked_sha256: BTreeMap<String, Option<String>>,
}

/// The candidate at `root` as it is now.
pub fn state(root: &Path) -> io::Result<Candidate> {
    let entries = status(root)?;
    let mut untracked = BTreeMap::new();
    for (code, rel) in &entries {
        if code == "??" {
            let p = root.join(OsStr::from_bytes(rel));
            let hash = if p.is_file() {
                Some(sha256_file(&p)?)
            } else {
                None
            };
            untracked.insert(String::from_utf8_lossy(rel).into_owned(), hash);
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
            .map(|(code, path)| format!("{code} {}", String::from_utf8_lossy(path)))
            .collect(),
        tracked_diff_sha256: sha256(&git(root, &["diff", "HEAD", "--binary"])?),
        untracked_sha256: untracked,
    })
}
