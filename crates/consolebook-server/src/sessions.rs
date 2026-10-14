//! Opaque server-side sessions.
//!
//! A session is a 256-bit random token handed to the browser in an `HttpOnly`
//! cookie; only its SHA-256 digest is stored. Sessions expire absolutely
//! and revoke immediately (logout, password reset).

use anyhow::{Context, Result};
use sqlx::{Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;

use crate::secrets::{self, OpaqueSecret};
use crate::storage;

/// Absolute session lifetime.
pub const SESSION_TTL_SECONDS: i64 = 12 * 60 * 60;

/// How the session's cookie travels: with the `Secure` attribute, or also
/// over plain HTTP (ADR 0025).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieTransport {
    Secure,
    Plain,
}

/// Creates a session for `user_id` and returns the raw token (for the
/// cookie) plus its expiry instant.
pub async fn create(
    pool: &SqlitePool,
    user_id: i64,
    transport: CookieTransport,
) -> Result<(OpaqueSecret, i64)> {
    let token = secrets::generate_session_token()?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let expires_at = now + SESSION_TTL_SECONDS;
    let mut tx = storage::write_tx(pool)
        .await
        .context("starting session creation")?;
    sqlx::query(
        "INSERT INTO session (token_hash, user_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&token.digest_hex)
    .bind(user_id)
    .bind(now)
    .bind(expires_at)
    .execute(&mut *tx)
    .await
    .context("creating session")?;
    if transport == CookieTransport::Secure {
        sqlx::query("INSERT INTO session_secure_cookie (token_hash) VALUES (?1)")
            .bind(&token.digest_hex)
            .execute(&mut *tx)
            .await
            .context("recording the session's Secure cookie")?;
    }
    tx.commit().await.context("committing session")?;

    // Opportunistic hygiene: drop sessions expired for more than a day so
    // the table does not accumulate forever. Validity never depends on
    // this; the lookup below always checks expiry itself.
    sqlx::query("DELETE FROM session WHERE expires_at < ?1")
        .bind(now - 86_400)
        .execute(pool)
        .await
        .context("pruning expired sessions")?;

    Ok((token, expires_at))
}

/// A live session, resolved from a presented token.
#[derive(Debug, Clone)]
pub struct LiveSession {
    pub user_id: i64,
    pub expires_at: i64,
}

/// Revokes every live session whose cookie was issued without `Secure`.
/// Called when the service starts with an `https://` public URL: such a
/// cookie may still sit in a browser that would send it over plain HTTP,
/// so it must stop being a credential now rather than at its expiry.
/// Returns how many were revoked.
pub async fn revoke_not_secure(pool: &SqlitePool) -> Result<u64> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let revoked = sqlx::query(
        "UPDATE session SET revoked_at = ?1
         WHERE revoked_at IS NULL AND expires_at > ?1
           AND token_hash NOT IN (SELECT token_hash FROM session_secure_cookie)",
    )
    .bind(now)
    .execute(pool)
    .await
    .context("revoking sessions issued without Secure")?;
    Ok(revoked.rows_affected())
}

/// Resolves a raw token to a live (unexpired, unrevoked) session.
pub async fn validate(pool: &SqlitePool, token: &str) -> Result<Option<LiveSession>> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT user_id, expires_at FROM session
         WHERE token_hash = ?1 AND revoked_at IS NULL AND expires_at > ?2",
    )
    .bind(secrets::digest_hex(token))
    .bind(now)
    .fetch_optional(pool)
    .await
    .context("validating session")?;
    Ok(row.map(|(user_id, expires_at)| LiveSession {
        user_id,
        expires_at,
    }))
}

/// Revokes the session behind a raw token. Idempotent.
pub async fn revoke(pool: &SqlitePool, token: &str) -> Result<()> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    sqlx::query("UPDATE session SET revoked_at = ?1 WHERE token_hash = ?2 AND revoked_at IS NULL")
        .bind(now)
        .bind(secrets::digest_hex(token))
        .execute(pool)
        .await
        .context("revoking session")?;
    Ok(())
}

/// Revokes every session for a user, inside the caller's transaction so it
/// commits atomically with the action that requires it (password reset).
pub async fn revoke_all_for_user(tx: &mut Transaction<'_, Sqlite>, user_id: i64) -> Result<()> {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    sqlx::query("UPDATE session SET revoked_at = ?1 WHERE user_id = ?2 AND revoked_at IS NULL")
        .bind(now)
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .context("revoking user sessions")?;
    Ok(())
}
