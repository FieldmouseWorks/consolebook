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

/// Every credential the ledger excludes, read from the database by the
/// exclusion list itself: a dotted exclusion's column, and every text
/// column of an excluded table that is not a closed vocabulary (a
/// `CHECK ... IN` list such as `issued_via` names a kind, not a secret).
async fn excluded_values(fx: &Fixture) -> Vec<(&'static str, Vec<String>)> {
    let mut out = Vec::new();
    for exclusion in &spec::EXCLUSIONS {
        let (table, columns) = if let Some((table, column)) = exclusion.name.split_once('.') {
            (table, vec![column.to_owned()])
        } else {
            {
                let sql: String =
                    sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name = ?1")
                        .bind(exclusion.name)
                        .fetch_one(&fx.pool)
                        .await
                        .expect("table sql");
                let vocabularies = spec::in_lists(&sql);
                let text: Vec<String> = sqlx::query_scalar(
                    "SELECT name FROM pragma_table_info(?1) WHERE type = 'TEXT'",
                )
                .bind(exclusion.name)
                .fetch_all(&fx.pool)
                .await
                .expect("columns");
                let secret = text
                    .into_iter()
                    .filter(|column| !vocabularies.contains_key(column))
                    .collect();
                (exclusion.name, secret)
            }
        };
        let mut values = Vec::new();
        for column in columns {
            let found: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL"
            ))
            .fetch_all(&fx.pool)
            .await
            .expect("credentials");
            values.extend(found);
        }
        out.push((exclusion.name, values));
    }
    out
}

#[tokio::test]
async fn credentials_never_leave_the_installation() {
    let p = populate().await;
    let fx = &p.fx;
    let bytes = export(fx, fx.admin_id).await;
    assert!(
        contains(&bytes, "avery.admin"),
        "the search sees stored text"
    );
    for (name, mut values) in excluded_values(fx).await {
        if name == "setup_code" {
            // Setup deletes its code in the transaction that uses it; the
            // fixture read the stored hash before that.
            values.push(fx.setup_code_hash.clone());
        }
        // Positive control: the credential is stored, so its absence from
        // the archive is a property of the export, not of the fixture.
        assert!(!values.is_empty(), "{name} holds nothing to look for");
        for value in &values {
            assert!(
                !contains(&bytes, value),
                "a {name} value left in the archive"
            );
        }
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

/// The producer applies the verifier's row rules: an audit subject with a
/// kind and no positive id, which verification would refuse, is refused
/// at production instead, naming the table and the row's key — never
/// exported as a complete archive that then fails verification.
#[tokio::test]
async fn a_row_the_verifier_would_refuse_is_refused_at_production() {
    for subject_id in [None, Some(0)] {
        let fx = Fixture::new().await;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO audit_event (occurred_at, kind, actor_user_id, subject_kind, subject_id)
             VALUES (1, 'program_created', ?1, 'program', ?2) RETURNING id",
        )
        .bind(fx.admin_id)
        .bind(subject_id)
        .fetch_one(&fx.pool)
        .await
        .expect("raw audit row");
        let err = installation_export::export_at(&fx.pool, fx.admin_id, EXPORTED_AT)
            .await
            .expect_err("refused at production");
        let message = format!("{err:#}");
        assert!(message.contains("table audit_event"), "{message}");
        assert!(message.contains(&format!("id={id}")), "{message}");
        assert!(message.contains("subject_kind and subject_id"), "{message}");
    }
}
