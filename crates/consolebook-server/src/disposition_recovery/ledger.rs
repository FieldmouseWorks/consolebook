//! Durable, canonical, installation-scoped disposition recovery ledger
//! (`data/instance/disposition-ledger.json`, mirrored to
//! `CONSOLEBOOK_BACKUP_COPY_DIR/disposition-ledger.json`; ADR 0030 §6; #213).
//!
//! Stores only opaque IDs, SHA-256 content hashes, UTC disposition timestamps,
//! and bounded agency policy authority references — zero record content,
//! narratives, ratings, acknowledgments, or personal names.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::SqlitePool;

use crate::backup;
use crate::canonical;
use crate::data_dir::DataDir;
use crate::disposition_evidence::AUTHENTICITY_SELF_CONSISTENCY_ONLY;
use crate::storage;

use super::verify_copy_dir_reachable_for_recovery;

/// File name under `data/instance/` and `CONSOLEBOOK_BACKUP_COPY_DIR/`.
pub const DISPOSITION_LEDGER_FILENAME: &str = "disposition-ledger.json";
/// Format discriminator; never changes.
pub const DISPOSITION_LEDGER_FORMAT: &str = "consolebook-disposition-ledger";
/// Bumped by any change to the recovery ledger schema or rules.
pub const DISPOSITION_LEDGER_FORMAT_VERSION: i64 = 1;

const ROOT_ALLOWLIST: &[&str] = &[
    "authenticity",
    "entries",
    "format",
    "format_version",
    "installation_id",
    "updated_at",
];

const ENTRY_ALLOWLIST: &[&str] = &[
    "content_hashes",
    "disposed_at",
    "disposition_run_id",
    "evaluation_record_id",
    "policy_authority",
];

/// Minimal, non-sensitive resurrection-prevention entry for one lawfully
/// disposed evaluation record lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispositionLedgerEntry {
    pub disposition_run_id: i64,
    pub evaluation_record_id: i64,
    pub content_hashes: Vec<String>,
    pub disposed_at: i64,
    pub policy_authority: String,
}

/// Installation-scoped recovery ledger stored in canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispositionLedger {
    pub format: String,
    pub format_version: i64,
    pub installation_id: String,
    pub authenticity: String,
    pub updated_at: i64,
    pub entries: Vec<DispositionLedgerEntry>,
}

impl DispositionLedger {
    /// Constructs a canonical v1 ledger with sorted, deduplicated entries and
    /// sorted, deduplicated content hashes.
    #[must_use]
    pub fn new(
        installation_id: impl Into<String>,
        updated_at: i64,
        mut entries: Vec<DispositionLedgerEntry>,
    ) -> Self {
        for entry in &mut entries {
            entry.content_hashes.sort();
            entry.content_hashes.dedup();
        }
        entries.sort_by_key(|entry| entry.evaluation_record_id);
        Self {
            format: DISPOSITION_LEDGER_FORMAT.to_owned(),
            format_version: DISPOSITION_LEDGER_FORMAT_VERSION,
            installation_id: installation_id.into(),
            authenticity: AUTHENTICITY_SELF_CONSISTENCY_ONLY.to_owned(),
            updated_at,
            entries,
        }
    }
}

/// Typed failure modes when loading, verifying, merging, or mirroring a
/// disposition recovery ledger. Every variant fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    Io {
        path: PathBuf,
        detail: String,
    },
    UnreadableJson {
        path: PathBuf,
        detail: String,
    },
    NotCanonical {
        path: PathBuf,
    },
    UnsupportedFormat {
        path: PathBuf,
        format: String,
        format_version: i64,
    },
    UnsupportedAuthenticity {
        path: PathBuf,
        authenticity: String,
    },
    DisallowedOrUnknownField {
        path: PathBuf,
        field: String,
    },
    InvalidContent {
        path: PathBuf,
        detail: String,
    },
    InstallationMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    CopyDirUnavailable {
        directory: PathBuf,
        problem: String,
    },
    StaleOrMissingLocalLedger {
        missing_record_ids: Vec<i64>,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, detail } => {
                write!(f, "reading disposition ledger {}: {detail}", path.display())
            }
            Self::UnreadableJson { path, detail } => {
                write!(
                    f,
                    "disposition ledger {} is not valid JSON: {detail}",
                    path.display()
                )
            }
            Self::NotCanonical { path } => {
                write!(
                    f,
                    "disposition ledger {} is not canonical JSON",
                    path.display()
                )
            }
            Self::UnsupportedFormat {
                path,
                format,
                format_version,
            } => write!(
                f,
                "disposition ledger {} has unsupported format '{format}' version {format_version}",
                path.display()
            ),
            Self::UnsupportedAuthenticity { path, authenticity } => write!(
                f,
                "disposition ledger {} declares unsupported authenticity '{authenticity}'",
                path.display()
            ),
            Self::DisallowedOrUnknownField { path, field } => write!(
                f,
                "disposition ledger {} carries disallowed or unknown field '{field}'",
                path.display()
            ),
            Self::InvalidContent { path, detail } => {
                write!(
                    f,
                    "disposition ledger {} is invalid: {detail}",
                    path.display()
                )
            }
            Self::InstallationMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "disposition ledger {} belongs to installation {actual}, not {expected}",
                path.display()
            ),
            Self::CopyDirUnavailable { directory, problem } => write!(
                f,
                "backup copy directory {} is unavailable for disposition recovery check: {problem}",
                directory.display()
            ),
            Self::StaleOrMissingLocalLedger { missing_record_ids } => write!(
                f,
                "local disposition recovery ledger is missing or stale: database records \
                 disposed evaluation_record id(s) {missing_record_ids:?} absent from \
                 consulted recovery ledger files"
            ),
        }
    }
}

impl std::error::Error for LedgerError {}

/// Result of inspecting one SQLite snapshot against a recovery ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDispositionCheck {
    pub installation_id: String,
    pub disposed_record_ids_present: Vec<i64>,
    pub disposed_content_hashes_present: Vec<String>,
}

impl SnapshotDispositionCheck {
    #[must_use]
    pub fn contains_disposed_content(&self) -> bool {
        !self.disposed_record_ids_present.is_empty()
            || !self.disposed_content_hashes_present.is_empty()
    }
}

/// Validates and serializes a ledger to canonical JSON bytes.
pub fn encode_canonical(ledger: &DispositionLedger) -> Result<Vec<u8>> {
    let value = serde_json::to_value(ledger).context("serializing disposition ledger")?;
    let bytes =
        canonical::canonical_bytes(&value).context("canonicalizing disposition ledger JSON")?;
    parse_and_verify(Path::new("<generated>"), &bytes)
        .context("generated disposition ledger failed self-verification")?;
    Ok(bytes)
}

/// Parses and strictly verifies a canonical recovery ledger document.
pub fn parse_and_verify(
    path: &Path,
    bytes: &[u8],
) -> std::result::Result<DispositionLedger, LedgerError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|err| LedgerError::UnreadableJson {
            path: path.to_path_buf(),
            detail: err.to_string(),
        })?;
    let Some(root) = value.as_object() else {
        return Err(LedgerError::InvalidContent {
            path: path.to_path_buf(),
            detail: "root must be a JSON object".into(),
        });
    };

    // Check format & version before allowlists so unknown versions fail by name.
    let format = root.get("format").and_then(Value::as_str);
    let format_version = root.get("format_version").and_then(Value::as_i64);
    match (format, format_version) {
        (Some(DISPOSITION_LEDGER_FORMAT), Some(DISPOSITION_LEDGER_FORMAT_VERSION)) => {}
        (Some(fmt_name), Some(version)) => {
            return Err(LedgerError::UnsupportedFormat {
                path: path.to_path_buf(),
                format: fmt_name.to_owned(),
                format_version: version,
            });
        }
        _ => {
            return Err(LedgerError::InvalidContent {
                path: path.to_path_buf(),
                detail: "format (string) and format_version (integer) are required".into(),
            });
        }
    }

    if canonical::canonical_bytes(&value).ok().as_deref() != Some(bytes) {
        return Err(LedgerError::NotCanonical {
            path: path.to_path_buf(),
        });
    }

    check_allowlist(path, root, ROOT_ALLOWLIST)?;
    if let Some(Value::Array(items)) = root.get("entries") {
        for item in items {
            if let Some(obj) = item.as_object() {
                check_allowlist(path, obj, ENTRY_ALLOWLIST)?;
            }
        }
    }

    let ledger: DispositionLedger =
        serde_json::from_value(value).map_err(|err| LedgerError::InvalidContent {
            path: path.to_path_buf(),
            detail: err.to_string(),
        })?;

    if ledger.authenticity != AUTHENTICITY_SELF_CONSISTENCY_ONLY {
        return Err(LedgerError::UnsupportedAuthenticity {
            path: path.to_path_buf(),
            authenticity: ledger.authenticity,
        });
    }
    if ledger.installation_id.trim().is_empty() {
        return Err(LedgerError::InvalidContent {
            path: path.to_path_buf(),
            detail: "installation_id must not be blank".into(),
        });
    }

    validate_entries(path, &ledger.entries)?;
    Ok(ledger)
}

fn check_allowlist(
    path: &Path,
    obj: &Map<String, Value>,
    allowlist: &[&str],
) -> std::result::Result<(), LedgerError> {
    for key in obj.keys() {
        if !allowlist.contains(&key.as_str()) {
            return Err(LedgerError::DisallowedOrUnknownField {
                path: path.to_path_buf(),
                field: key.clone(),
            });
        }
    }
    Ok(())
}

fn validate_entries(
    path: &Path,
    entries: &[DispositionLedgerEntry],
) -> std::result::Result<(), LedgerError> {
    let mut prev_record_id: Option<i64> = None;
    for entry in entries {
        if entry.disposition_run_id <= 0 || entry.evaluation_record_id <= 0 {
            return Err(LedgerError::InvalidContent {
                path: path.to_path_buf(),
                detail: "disposition_run_id and evaluation_record_id must be positive".into(),
            });
        }
        if let Some(prev) = prev_record_id
            && entry.evaluation_record_id <= prev
        {
            return Err(LedgerError::InvalidContent {
                path: path.to_path_buf(),
                detail: "entries must be unique and strictly ascending by evaluation_record_id"
                    .into(),
            });
        }
        prev_record_id = Some(entry.evaluation_record_id);

        let auth_len = entry.policy_authority.trim().chars().count();
        if auth_len == 0 || auth_len > 200 {
            return Err(LedgerError::InvalidContent {
                path: path.to_path_buf(),
                detail: "policy_authority must contain 1..=200 non-blank characters".into(),
            });
        }

        let mut prev_hash: Option<&str> = None;
        for hash in &entry.content_hashes {
            if !is_sha256_hex(hash) {
                return Err(LedgerError::InvalidContent {
                    path: path.to_path_buf(),
                    detail: format!(
                        "record {} carries malformed content_hash",
                        entry.evaluation_record_id
                    ),
                });
            }
            if let Some(prev) = prev_hash
                && hash.as_str() <= prev
            {
                return Err(LedgerError::InvalidContent {
                    path: path.to_path_buf(),
                    detail: format!(
                        "record {} content_hashes must be unique and strictly ascending",
                        entry.evaluation_record_id
                    ),
                });
            }
            prev_hash = Some(hash.as_str());
        }
    }
    Ok(())
}

/// Reads and verifies a ledger file if it exists. Returns `Ok(None)` only when
/// the path does not exist; any other I/O or format error fails closed.
pub fn read_ledger_if_present(
    path: &Path,
) -> std::result::Result<Option<DispositionLedger>, LedgerError> {
    match std::fs::read(path) {
        Ok(bytes) => parse_and_verify(path, &bytes).map(Some),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(LedgerError::Io {
            path: path.to_path_buf(),
            detail: err.to_string(),
        }),
    }
}

/// Reads and verifies a required ledger file (such as `--disposition-ledger`).
/// Missing files fail closed.
pub fn read_required_ledger(path: &Path) -> std::result::Result<DispositionLedger, LedgerError> {
    let bytes = std::fs::read(path).map_err(|err| LedgerError::Io {
        path: path.to_path_buf(),
        detail: err.to_string(),
    })?;
    parse_and_verify(path, &bytes)
}

/// Atomically writes and fsyncs a canonical recovery ledger (temp file +
/// `sync_all` + rename + file/directory fsync).
pub fn write_ledger_atomic(path: &Path, ledger: &DispositionLedger) -> Result<()> {
    let bytes = encode_canonical(ledger)?;
    let parent = path
        .parent()
        .context("disposition ledger path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating ledger directory {}", parent.display()))?;
    let name = path
        .file_name()
        .context("disposition ledger path has no file name")?
        .to_string_lossy();
    let tmp_path = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let write_res = (|| -> Result<()> {
        let mut file = File::create(&tmp_path)
            .with_context(|| format!("creating temporary ledger {}", tmp_path.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("writing temporary ledger {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing temporary ledger {}", tmp_path.display()))?;
        drop(file);
        std::fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "renaming temporary ledger {} into place as {}",
                tmp_path.display(),
                path.display()
            )
        })?;
        backup::persist(path)?;
        Ok(())
    })();
    if write_res.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    write_res
}

/// Merges one or more ledgers belonging to `installation_id` into one
/// deterministic canonical ledger (union of disposed records and their content
/// hashes). Fails closed if any input ledger belongs to another installation.
pub fn merge_ledgers(
    installation_id: &str,
    ledgers: &[&DispositionLedger],
) -> std::result::Result<DispositionLedger, LedgerError> {
    let mut by_record: BTreeMap<i64, DispositionLedgerEntry> = BTreeMap::new();
    let mut updated_at = 0_i64;
    for ledger in ledgers {
        if ledger.installation_id != installation_id {
            return Err(LedgerError::InstallationMismatch {
                path: PathBuf::from("<merged>"),
                expected: installation_id.to_owned(),
                actual: ledger.installation_id.clone(),
            });
        }
        updated_at = updated_at.max(ledger.updated_at);
        for entry in &ledger.entries {
            by_record
                .entry(entry.evaluation_record_id)
                .and_modify(|existing| {
                    let mut hashes: BTreeSet<String> = existing.content_hashes.drain(..).collect();
                    hashes.extend(entry.content_hashes.iter().cloned());
                    existing.content_hashes = hashes.into_iter().collect();
                    if (entry.disposed_at, entry.disposition_run_id)
                        < (existing.disposed_at, existing.disposition_run_id)
                    {
                        existing.disposed_at = entry.disposed_at;
                        existing.disposition_run_id = entry.disposition_run_id;
                        existing
                            .policy_authority
                            .clone_from(&entry.policy_authority);
                    }
                })
                .or_insert_with(|| entry.clone());
        }
    }
    Ok(DispositionLedger::new(
        installation_id,
        updated_at,
        by_record.into_values().collect(),
    ))
}

/// Builds a recovery ledger snapshot from `disposition_event` and
/// `disposition_version_hash` rows in `pool`. Read-only; safe on diagnostic
/// and immutable snapshot connections, and returns an empty ledger on pre-0018
/// schemas that do not yet have `disposition_event`.
pub async fn build_ledger_from_db(pool: &SqlitePool, updated_at: i64) -> Result<DispositionLedger> {
    let installation_id = storage::installation_id(pool).await?;
    let has_table: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'disposition_event')",
    )
    .fetch_one(pool)
    .await
    .context("checking for disposition_event table")?;
    if has_table == 0 {
        return Ok(DispositionLedger::new(
            installation_id,
            updated_at,
            Vec::new(),
        ));
    }

    let event_rows: Vec<(i64, i64, i64, i64, String)> = sqlx::query_as(
        "SELECT id, disposition_run_id, evaluation_record_id, disposed_at, authority_reference \
         FROM disposition_event \
         ORDER BY evaluation_record_id ASC",
    )
    .fetch_all(pool)
    .await
    .context("reading disposition_event rows")?;

    let hash_rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT disposition_event_id, content_hash \
         FROM disposition_version_hash \
         ORDER BY disposition_event_id ASC, content_hash ASC",
    )
    .fetch_all(pool)
    .await
    .context("reading disposition_version_hash rows")?;

    let mut hashes_by_event: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for (event_id, content_hash) in hash_rows {
        hashes_by_event
            .entry(event_id)
            .or_default()
            .push(content_hash);
    }

    let mut max_disposed = updated_at;
    let entries = event_rows
        .into_iter()
        .map(
            |(
                event_id,
                disposition_run_id,
                evaluation_record_id,
                disposed_at,
                policy_authority,
            )| {
                max_disposed = max_disposed.max(disposed_at);
                DispositionLedgerEntry {
                    disposition_run_id,
                    evaluation_record_id,
                    content_hashes: hashes_by_event.remove(&event_id).unwrap_or_default(),
                    disposed_at,
                    policy_authority,
                }
            },
        )
        .collect();

    Ok(DispositionLedger::new(
        installation_id,
        max_disposed,
        entries,
    ))
}

/// Synchronizes `data/instance/disposition-ledger.json` with `pool` by merging
/// any existing local ledger with the database's `disposition_event` rows and
/// persisting atomically.
pub async fn sync_local_ledger_from_db(
    data_dir: &DataDir,
    pool: &SqlitePool,
    updated_at: i64,
) -> Result<DispositionLedger> {
    let from_db = build_ledger_from_db(pool, updated_at).await?;
    let local_path = data_dir.disposition_ledger();
    let merged = match read_ledger_if_present(&local_path)? {
        Some(existing) => {
            let mut m = merge_ledgers(&from_db.installation_id, &[&existing, &from_db])?;
            m.updated_at = m.updated_at.max(updated_at);
            m
        }
        None => from_db,
    };
    write_ledger_atomic(&local_path, &merged)?;
    Ok(merged)
}

/// Mirrors `ledger` into `copy_dir/disposition-ledger.json` after verifying
/// that `copy_dir` is reachable on another filesystem and belongs to the same
/// installation. Merges with any existing mirror for the same installation so
/// off-server recovery evidence never regresses.
pub async fn mirror_ledger_to_copy_dir(
    data_dir: &DataDir,
    copy_dir: &Path,
    ledger: &DispositionLedger,
) -> Result<PathBuf> {
    verify_copy_dir_reachable_for_recovery(copy_dir, &data_dir.backups())?;

    // Check installation ownership against existing snapshots in copy_dir.
    let mut existing_snapshots = backup::own_backups_in(copy_dir)?;
    existing_snapshots.reverse();
    for snap in &existing_snapshots {
        if let Ok(other_id) = backup::snapshot_installation(snap).await {
            if other_id != ledger.installation_id {
                return Err(LedgerError::InstallationMismatch {
                    path: snap.clone(),
                    expected: ledger.installation_id.clone(),
                    actual: other_id,
                }
                .into());
            }
            break;
        }
    }

    let mirror_path = copy_dir.join(DISPOSITION_LEDGER_FILENAME);
    let merged = match read_ledger_if_present(&mirror_path)? {
        Some(existing) => {
            if existing.installation_id != ledger.installation_id {
                return Err(LedgerError::InstallationMismatch {
                    path: mirror_path,
                    expected: ledger.installation_id.clone(),
                    actual: existing.installation_id,
                }
                .into());
            }
            merge_ledgers(&ledger.installation_id, &[&existing, ledger])?
        }
        None => ledger.clone(),
    };
    write_ledger_atomic(&mirror_path, &merged)?;
    Ok(mirror_path)
}

/// Inspects a SQLite snapshot read-only (immutable, writing no sidecars) to
/// detect whether any disposed `evaluation_record.id` or
/// `evaluation_version.content_hash` in `ledger` is still present.
pub async fn inspect_snapshot_against_ledger(
    snapshot: &Path,
    ledger: &DispositionLedger,
) -> Result<SnapshotDispositionCheck> {
    let pool = storage::open_snapshot(snapshot).await?;
    let installation_id = storage::installation_id(&pool)
        .await
        .with_context(|| format!("reading installation_id of {}", snapshot.display()))?;

    if ledger.entries.is_empty() {
        pool.close().await;
        return Ok(SnapshotDispositionCheck {
            installation_id,
            disposed_record_ids_present: Vec::new(),
            disposed_content_hashes_present: Vec::new(),
        });
    }

    let ledger_ids: BTreeSet<i64> = ledger
        .entries
        .iter()
        .map(|entry| entry.evaluation_record_id)
        .collect();
    let ledger_hashes: BTreeSet<&str> = ledger
        .entries
        .iter()
        .flat_map(|entry| entry.content_hashes.iter().map(String::as_str))
        .collect();

    let mut disposed_record_ids_present = Vec::new();
    let has_records: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'evaluation_record')",
    )
    .fetch_one(&pool)
    .await
    .context("checking evaluation_record table in snapshot")?;
    if has_records != 0 {
        let present_ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM evaluation_record ORDER BY id ASC")
                .fetch_all(&pool)
                .await
                .context("reading evaluation_record ids from snapshot")?;
        for id in present_ids {
            if ledger_ids.contains(&id) {
                disposed_record_ids_present.push(id);
            }
        }
    }

    let mut disposed_content_hashes_present = Vec::new();
    if !ledger_hashes.is_empty() {
        let has_versions: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'evaluation_version')",
        )
        .fetch_one(&pool)
        .await
        .context("checking evaluation_version table in snapshot")?;
        if has_versions != 0 {
            let present_hashes: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT content_hash FROM evaluation_version ORDER BY content_hash ASC",
            )
            .fetch_all(&pool)
            .await
            .context("reading evaluation_version content hashes from snapshot")?;
            for hash in present_hashes {
                if ledger_hashes.contains(hash.as_str()) {
                    disposed_content_hashes_present.push(hash);
                }
            }
        }
    }

    pool.close().await;
    Ok(SnapshotDispositionCheck {
        installation_id,
        disposed_record_ids_present,
        disposed_content_hashes_present,
    })
}

/// Monotonically advances `evaluation_record_id_floor.last_reserved_id` to at
/// least the highest `evaluation_record_id` recorded in `ledger`, preventing
/// new drafts created after restoring an older snapshot from ever reusing a
/// ledger-listed disposed record ID (ADR 0030; #213).
pub async fn advance_record_id_floor(pool: &SqlitePool, ledger: &DispositionLedger) -> Result<()> {
    let Some(max_id) = ledger
        .entries
        .iter()
        .map(|entry| entry.evaluation_record_id)
        .max()
    else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE evaluation_record_id_floor \
         SET last_reserved_id = MAX(last_reserved_id, ?1) \
         WHERE id = 1",
    )
    .bind(max_id)
    .execute(pool)
    .await
    .context("advancing evaluation_record_id_floor from recovery ledger")?;
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
