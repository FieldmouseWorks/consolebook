//! What a receipt says about the machine and the files it ran on.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::WEB_BUILD;

/// Now, in UTC to the second: `2026-10-31T21:00:00Z`.
#[must_use]
pub fn now() -> String {
    jiff::Timestamp::now()
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Lowercase hex of a digest.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[must_use]
pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(hex(&h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

/// The first line a tool prints about itself, or why there's none.
#[must_use]
pub fn tool_version(argv: &[&str], cwd: &Path) -> String {
    let child = Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return format!("unavailable ({:?})", e.kind()),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return "unavailable (timed out)".to_owned();
            }
            Err(e) => return format!("unavailable ({:?})", e.kind()),
        }
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return format!("unavailable ({:?})", e.kind()),
    };
    let text = if out.stdout.is_empty() {
        out.stderr
    } else {
        out.stdout
    };
    let text = String::from_utf8_lossy(&text);
    match text.trim().lines().next() {
        Some(first) if out.status.success() => first.to_owned(),
        _ => format!(
            "unavailable (exit {})",
            out.status
                .code()
                .map_or_else(|| "by signal".to_owned(), |c| c.to_string())
        ),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Browser {
    pub kind: &'static str,
    pub path: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// The browser the suite ran with: the system one it was given, or the one
/// Playwright resolves.
#[must_use]
pub fn browser(root: &Path, chromium: Option<&str>) -> Browser {
    if let Some(path) = chromium {
        return Browser {
            kind: "system",
            path: path.to_owned(),
            version: tool_version(&[path, "--version"], root),
            note: None,
        };
    }
    let path = tool_version(
        &[
            "node",
            "-e",
            "process.stdout.write(require('@playwright/test').chromium.executablePath())",
        ],
        &root.join("web"),
    );
    let version = if path.starts_with('/') {
        tool_version(&[&path, "--version"], root)
    } else {
        "unresolved".to_owned()
    };
    Browser {
        kind: "playwright-managed",
        path,
        version,
        note: Some("path as resolved by Playwright; headless runs may use its headless shell"),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Host {
    pub platform: String,
    /// Whether this process can bypass file permissions, as the permission
    /// tests care: `CAP_DAC_OVERRIDE` or `CAP_DAC_READ_SEARCH` in its effective
    /// set, so a root process with those dropped counts as unprivileged;
    /// an effective uid of 0 where there's no capability set to read.
    pub privileged: bool,
    pub euid: u32,
    pub privilege_source: &'static str,
}

#[must_use]
pub fn host() -> Host {
    let uname = rustix::system::uname();
    let platform = format!(
        "{}-{}-{}",
        uname.sysname().to_string_lossy(),
        uname.release().to_string_lossy(),
        uname.machine().to_string_lossy()
    );
    let euid = rustix::process::geteuid().as_raw();
    let caps = fs::read_to_string("/proc/self/status").ok().and_then(|s| {
        s.lines()
            .find_map(|l| l.strip_prefix("CapEff:"))
            .and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
    });
    match caps {
        Some(caps) => Host {
            platform,
            privileged: caps & 0b110 != 0,
            euid,
            privilege_source: "CapEff",
        },
        None => Host {
            platform,
            privileged: euid == 0,
            euid,
            privilege_source: "euid",
        },
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Manifest {
    pub path: String,
    pub files: usize,
    pub sha256: String,
}

/// Every file of the web build, hashed into a manifest in the receipt's
/// directory; None if there's no build.
pub fn web_manifest(root: &Path, out_dir: &Path) -> io::Result<Option<Manifest>> {
    fn walk(root: &Path, dir: &Path, rows: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            // Directories it links to are left out; files it links to are in.
            if fs::symlink_metadata(&path)?.is_dir() {
                walk(root, &path, rows)?;
            } else if path.is_file() {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                rows.push(format!("{}  {}", sha256_file(&path)?, rel.display()));
            }
        }
        Ok(())
    }
    let build = root.join(WEB_BUILD);
    if !build.is_dir() {
        return Ok(None);
    }
    let mut rows = Vec::new();
    walk(root, &build, &mut rows)?;
    rows.sort();
    let manifest = out_dir.join("web-build.sha256");
    fs::write(&manifest, rows.join("\n") + "\n")?;
    Ok(Some(Manifest {
        path: "web-build.sha256".to_owned(),
        files: rows.len(),
        sha256: sha256_file(&manifest)?,
    }))
}
