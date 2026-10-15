//! Properties of a produced archive: a function of the rows and the
//! instant, never carrying a credential, and never passing through a
//! stored value the build does not understand.

use super::*;

#[tokio::test]
async fn the_export_is_a_function_of_the_rows_and_the_instant() {
    let p = populate().await;
    let fx = &p.fx;
    let first = entries(&export(fx, fx.admin_id).await);
    let second = entries(&export(fx, fx.admin_id).await);
    // Each export audits itself, and the audit trail is carried: the
    // second archive holds exactly one more audit row than the first,
    // and nothing else differs. Without that row the bytes are the same.
    assert_eq!(first.len(), second.len());
    let audit_path = installation_export::table_path("audit_event");
    for ((name, a), (other, b)) in first.iter().zip(&second) {
        assert_eq!(name, other);
        if name != ARCHIVE_MANIFEST_PATH && *name != audit_path {
            assert_eq!(a, b, "{name} differs between exports at one instant");
        }
    }
    let mut later = rows_of(&second, "audit_event");
    let added = later.pop().expect("the second export's audit");
    assert_eq!(added["kind"], "installation_exported");
    assert_eq!(
        canonical::canonical_bytes(&serde_json::Value::Array(later)).expect("canonical"),
        entry(&first, &audit_path),
        "the earlier rows are the earlier document, byte for byte"
    );
    let mut first_manifest = manifest_of(&first);
    let second_manifest = manifest_of(&second);
    for stated in &mut first_manifest.tables {
        if stated.name == "audit_event" {
            let again = second_manifest
                .tables
                .iter()
                .find(|entry| entry.name == "audit_event")
                .expect("listed");
            assert_eq!(again.rows, stated.rows + 1);
            *stated = again.clone();
        }
    }
    assert_eq!(first_manifest, second_manifest);
}

#[tokio::test]
async fn credentials_never_leave_the_installation() {
    let p = populate().await;
    let fx = &p.fx;
    let bytes = export(fx, fx.admin_id).await;
    let mut secrets: Vec<String> = Vec::new();
    for sql in [
        "SELECT password_hash FROM user",
        "SELECT token_hash FROM session",
        "SELECT code_hash FROM password_reset_code",
    ] {
        let found: Vec<String> = sqlx::query_scalar(sql)
            .fetch_all(&fx.pool)
            .await
            .expect("credentials");
        // Positive control: the credential is stored, so its absence from
        // the archive is a property of the export, not of the fixture.
        assert!(!found.is_empty(), "{sql} found nothing");
        secrets.extend(found);
    }
    secrets.push(fx.setup_code_hash.clone());
    assert!(
        contains(&bytes, "avery.admin"),
        "the search sees stored text"
    );
    for secret in &secrets {
        assert!(
            !contains(&bytes, secret),
            "a credential left in the archive"
        );
    }
    for name in ["password_hash", "token_hash", "code_hash"] {
        assert!(!contains(&bytes, &format!("\"{name}\"")), "{name} member");
    }
}

/// A stored value the build does not understand is refused at production,
/// never passed through: an unknown vocabulary value and a flag that is
/// neither 0 nor 1.
#[tokio::test]
async fn unknown_stored_values_are_refused_at_production() {
    let fx = Fixture::new().await;
    export(&fx, fx.admin_id).await;
    sqlx::query(
        "INSERT INTO notice (user_id, kind, message, created_at)
         VALUES (?1, 'invented_unknown_kind', 'Invented.', 1)",
    )
    .bind(fx.admin_id)
    .execute(&fx.pool)
    .await
    .expect("raw notice");
    let err = installation_export::export_at(&fx.pool, fx.admin_id, EXPORTED_AT)
        .await
        .expect_err("an unknown kind is refused");
    assert!(format!("{err:#}").contains("vocabulary"), "{err:#}");
    sqlx::query("DELETE FROM notice WHERE kind = 'invented_unknown_kind'")
        .execute(&fx.pool)
        .await
        .expect("cleanup");
    export(&fx, fx.admin_id).await;

    // Every flag is held to 0 or 1 by a CHECK or a trigger; dropping the
    // trigger on this scratch database stands in for a value written by a
    // build whose rules this one does not know.
    let p = populate().await;
    sqlx::query("DROP TRIGGER draft_rating_not_observed_shape_update")
        .execute(&p.fx.pool)
        .await
        .expect("drop the shape trigger");
    sqlx::query("UPDATE draft_rating SET value = NULL, not_observed = 2 WHERE id = (SELECT MAX(id) FROM draft_rating)")
        .execute(&p.fx.pool)
        .await
        .expect("raw flag on the open draft");
    let err = installation_export::export_at(&p.fx.pool, p.fx.admin_id, EXPORTED_AT)
        .await
        .expect_err("a flag of 2 is refused");
    assert!(format!("{err:#}").contains("flag"), "{err:#}");
}
