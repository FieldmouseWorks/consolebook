//! Versioned program configuration: programs, immutable published program
//! versions, and the typed content a version owns (ADR 0007).
//!
//! A draft version is edited by wholesale content replacement — single
//! editor with honest last-write behavior. Publishing freezes the version;
//! after that the database itself (migration 0004) rejects every mutation
//! of the version and its owned rows. Content is validated before any row
//! is written, so the composite foreign keys that enforce domain
//! invariant 5 never see a dangling reference.

mod content;
mod persistence;

pub use content::*;
pub use persistence::load_content;
use persistence::{delete_content, insert_content};
pub(crate) use persistence::{insert_program, insert_version};

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::audit::{self, EventKind, Subject};
use crate::capabilities::{self, Capability};
use crate::storage;

// ---- summaries

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProgramSummary {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionSummary {
    pub id: i64,
    pub program_id: i64,
    pub version_number: i64,
    pub label: String,
    pub name: String,
    pub created_at: i64,
    pub published_at: Option<i64>,
}

// ---- refusals

/// Why creating a program was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum ProgramRefusal {
    CapabilityRequired,
    NameEmpty,
    NameTaken,
}

/// Why an authoring operation on a version was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthorRefusal {
    CapabilityRequired,
    NoSuchProgram,
    NoSuchVersion,
    AlreadyPublished,
    Invalid(Vec<String>),
}

/// Why publishing was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PublishRefusal {
    CapabilityRequired,
    NoSuchVersion,
    AlreadyPublished,
    Incomplete(Vec<String>),
}

// ---- services

async fn holds_manage_programs(pool: &SqlitePool, user_id: i64) -> Result<bool> {
    capabilities::user_has(pool, user_id, Capability::ManagePrograms).await
}

/// Creates a program identity. The name is the mutable discovery name;
/// each version snapshots the name it presents.
pub async fn create_program(
    pool: &SqlitePool,
    actor_user_id: i64,
    name: &str,
) -> Result<std::result::Result<i64, ProgramRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(ProgramRefusal::CapabilityRequired));
    }
    let name = name.trim();
    if name.is_empty() {
        return Ok(Err(ProgramRefusal::NameEmpty));
    }
    let mut tx = storage::write_tx(pool)
        .await
        .context("starting program creation")?;
    let taken: Option<i64> =
        sqlx::query_scalar("SELECT id FROM program WHERE name = ?1 COLLATE NOCASE")
            .bind(name)
            .fetch_optional(&mut *tx)
            .await
            .context("checking program name")?;
    if taken.is_some() {
        return storage::refuse(tx, ProgramRefusal::NameTaken).await;
    }
    let program_id = insert_program(&mut tx, name, actor_user_id).await?;
    tx.commit().await.context("committing program creation")?;
    Ok(Ok(program_id))
}

pub async fn list_programs(pool: &SqlitePool) -> Result<Vec<ProgramSummary>> {
    let rows = sqlx::query("SELECT id, name, created_at FROM program ORDER BY name COLLATE NOCASE")
        .fetch_all(pool)
        .await
        .context("listing programs")?;
    Ok(rows
        .iter()
        .map(|row| ProgramSummary {
            id: row.get("id"),
            name: row.get("name"),
            created_at: row.get("created_at"),
        })
        .collect())
}

/// Looks up one program's summary.
pub async fn get_program(pool: &SqlitePool, program_id: i64) -> Result<Option<ProgramSummary>> {
    let row = sqlx::query("SELECT id, name, created_at FROM program WHERE id = ?1")
        .bind(program_id)
        .fetch_optional(pool)
        .await
        .context("looking up program")?;
    Ok(row.as_ref().map(|row| ProgramSummary {
        id: row.get("id"),
        name: row.get("name"),
        created_at: row.get("created_at"),
    }))
}

/// Looks up one version's summary.
pub async fn version_summary(pool: &SqlitePool, version_id: i64) -> Result<Option<VersionSummary>> {
    let row = sqlx::query(
        "SELECT id, program_id, version_number, label, name, created_at, published_at
         FROM program_version WHERE id = ?1",
    )
    .bind(version_id)
    .fetch_optional(pool)
    .await
    .context("looking up program version")?;
    Ok(row.as_ref().map(|row| VersionSummary {
        id: row.get("id"),
        program_id: row.get("program_id"),
        version_number: row.get("version_number"),
        label: row.get("label"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        published_at: row.get("published_at"),
    }))
}

pub async fn list_versions(pool: &SqlitePool, program_id: i64) -> Result<Vec<VersionSummary>> {
    let rows = sqlx::query(
        "SELECT id, program_id, version_number, label, name, created_at, published_at
         FROM program_version WHERE program_id = ?1 ORDER BY version_number",
    )
    .bind(program_id)
    .fetch_all(pool)
    .await
    .context("listing program versions")?;
    Ok(rows
        .iter()
        .map(|row| VersionSummary {
            id: row.get("id"),
            program_id: row.get("program_id"),
            version_number: row.get("version_number"),
            label: row.get("label"),
            name: row.get("name"),
            created_at: row.get("created_at"),
            published_at: row.get("published_at"),
        })
        .collect())
}

/// Creates a draft version of `program_id` holding `content`, assigning
/// the next monotonic version number.
pub async fn create_version(
    pool: &SqlitePool,
    actor_user_id: i64,
    program_id: i64,
    content: &VersionContent,
) -> Result<std::result::Result<i64, AuthorRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(AuthorRefusal::CapabilityRequired));
    }
    let problems = validate_content(content);
    if !problems.is_empty() {
        return Ok(Err(AuthorRefusal::Invalid(problems)));
    }
    let mut tx = storage::write_tx(pool)
        .await
        .context("starting version creation")?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM program WHERE id = ?1")
        .bind(program_id)
        .fetch_optional(&mut *tx)
        .await
        .context("checking program")?;
    if exists.is_none() {
        return storage::refuse(tx, AuthorRefusal::NoSuchProgram).await;
    }
    let version_id = insert_version(
        &mut tx,
        program_id,
        content,
        actor_user_id,
        EventKind::ProgramVersionCreated,
    )
    .await?;
    tx.commit().await.context("committing version creation")?;
    Ok(Ok(version_id))
}

/// Replaces a draft's entire content — single editor, honest last write
/// (ADR 0007). Refused once the version is published.
pub async fn replace_draft(
    pool: &SqlitePool,
    actor_user_id: i64,
    version_id: i64,
    content: &VersionContent,
) -> Result<std::result::Result<(), AuthorRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(AuthorRefusal::CapabilityRequired));
    }
    let problems = validate_content(content);
    if !problems.is_empty() {
        return Ok(Err(AuthorRefusal::Invalid(problems)));
    }
    let mut tx = storage::write_tx(pool)
        .await
        .context("starting draft replacement")?;
    match version_state(&mut tx, version_id).await? {
        VersionState::Missing => return storage::refuse(tx, AuthorRefusal::NoSuchVersion).await,
        VersionState::Published => {
            return storage::refuse(tx, AuthorRefusal::AlreadyPublished).await;
        }
        VersionState::Draft => {}
    }
    delete_content(&mut tx, version_id).await?;
    sqlx::query("UPDATE program_version SET label = ?2, name = ?3, description = ?4 WHERE id = ?1")
        .bind(version_id)
        .bind(&content.label)
        .bind(&content.name)
        .bind(&content.description)
        .execute(&mut *tx)
        .await
        .context("updating version row")?;
    insert_content(&mut tx, version_id, content).await?;
    tx.commit().await.context("committing draft replacement")?;
    Ok(Ok(()))
}

/// Publishes a draft: completeness-checks it, stamps `published_at`, and
/// leaves all further mutation to be refused by the database.
pub async fn publish_version(
    pool: &SqlitePool,
    actor_user_id: i64,
    version_id: i64,
) -> Result<std::result::Result<(), PublishRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(PublishRefusal::CapabilityRequired));
    }
    let mut tx = storage::write_tx(pool).await.context("starting publish")?;
    match version_state(&mut tx, version_id).await? {
        VersionState::Missing => return storage::refuse(tx, PublishRefusal::NoSuchVersion).await,
        VersionState::Published => {
            return storage::refuse(tx, PublishRefusal::AlreadyPublished).await;
        }
        VersionState::Draft => {}
    }
    let empty_forms: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM evaluation_form
         WHERE program_version_id = ?1
           AND id NOT IN (SELECT evaluation_form_id FROM form_competency WHERE program_version_id = ?1)
           AND id NOT IN (SELECT evaluation_form_id FROM form_narrative WHERE program_version_id = ?1)
         ORDER BY name",
    )
    .bind(version_id)
    .fetch_all(&mut *tx)
    .await
    .context("checking form completeness")?;
    if !empty_forms.is_empty() {
        let problems = empty_forms
            .iter()
            .map(|name| format!("form '{name}' has no competencies and no narratives"))
            .collect();
        return storage::refuse(tx, PublishRefusal::Incomplete(problems)).await;
    }
    let stamped = sqlx::query(
        "UPDATE program_version SET published_at = ?2, published_by = ?3
         WHERE id = ?1 AND published_at IS NULL",
    )
    .bind(version_id)
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await
    .context("publishing version")?;
    if stamped.rows_affected() != 1 {
        return storage::refuse(tx, PublishRefusal::AlreadyPublished).await;
    }
    audit::record_for_subject(
        &mut *tx,
        EventKind::ProgramVersionPublished,
        Some(actor_user_id),
        None,
        Subject::ProgramVersion(version_id),
    )
    .await?;
    tx.commit().await.context("committing publish")?;
    Ok(Ok(()))
}

/// Deletes a draft version and its content. Published versions are
/// immutable and refuse this at the database as well as here.
pub async fn discard_draft(
    pool: &SqlitePool,
    actor_user_id: i64,
    version_id: i64,
) -> Result<std::result::Result<(), AuthorRefusal>> {
    if !holds_manage_programs(pool, actor_user_id).await? {
        return Ok(Err(AuthorRefusal::CapabilityRequired));
    }
    let mut tx = storage::write_tx(pool)
        .await
        .context("starting draft discard")?;
    match version_state(&mut tx, version_id).await? {
        VersionState::Missing => return storage::refuse(tx, AuthorRefusal::NoSuchVersion).await,
        VersionState::Published => {
            return storage::refuse(tx, AuthorRefusal::AlreadyPublished).await;
        }
        VersionState::Draft => {}
    }
    delete_content(&mut tx, version_id).await?;
    sqlx::query("DELETE FROM program_version WHERE id = ?1")
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .context("deleting draft version")?;
    audit::record_for_subject(
        &mut *tx,
        EventKind::ProgramVersionDiscarded,
        Some(actor_user_id),
        None,
        Subject::ProgramVersion(version_id),
    )
    .await?;
    tx.commit().await.context("committing draft discard")?;
    Ok(Ok(()))
}

enum VersionState {
    Missing,
    Draft,
    Published,
}

async fn version_state(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
) -> Result<VersionState> {
    let row = sqlx::query("SELECT published_at FROM program_version WHERE id = ?1")
        .bind(version_id)
        .fetch_optional(&mut **tx)
        .await
        .context("checking version state")?;
    Ok(match row {
        None => VersionState::Missing,
        Some(row) => {
            let published_at: Option<i64> = row.get("published_at");
            if published_at.is_some() {
                VersionState::Published
            } else {
                VersionState::Draft
            }
        }
    })
}
