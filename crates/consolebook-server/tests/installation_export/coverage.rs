//! The format's table and column lists are enforced, not just written
//! down: the coverage guard compares the migrated schema with the table
//! specification both ways. Each control below changes one thing on a
//! scratch database and names the finding it must produce.

use consolebook_server::installation_export::spec::{
    self as table_spec, CoverageFinding, Exclusion, ExclusionReason, coverage_findings,
    specification_findings,
};

use super::*;

/// A freshly migrated scratch database.
async fn scratch() -> (tempfile::TempDir, sqlx::SqlitePool) {
    let tmp = tempfile::tempdir().expect("scratch");
    let pool = storage::open(&tmp.path().join("consolebook.db"))
        .await
        .expect("migrate");
    (tmp, pool)
}

async fn findings(pool: &sqlx::SqlitePool) -> Vec<CoverageFinding> {
    let mut conn = pool.acquire().await.expect("connection");
    coverage_findings(&mut conn).await.expect("coverage")
}

#[tokio::test]
async fn the_specification_covers_the_migrated_schema() {
    let (_tmp, pool) = scratch().await;
    assert_eq!(findings(&pool).await, Vec::new());
}

#[tokio::test]
async fn a_table_or_column_the_format_does_not_decide_fails_coverage() {
    let (_tmp, pool) = scratch().await;
    sqlx::query("CREATE TABLE invented_extra (id INTEGER PRIMARY KEY, note TEXT NOT NULL) STRICT")
        .execute(&pool)
        .await
        .expect("extra table");
    sqlx::query("ALTER TABLE notice ADD COLUMN invented_note TEXT")
        .execute(&pool)
        .await
        .expect("extra column");
    sqlx::query("ALTER TABLE user DROP COLUMN title")
        .execute(&pool)
        .await
        .expect("dropped column");
    let found = findings(&pool).await;
    for expected in [
        CoverageFinding::TableNotInSpec {
            table: "invented_extra".to_owned(),
        },
        CoverageFinding::ColumnNotInSpec {
            table: "notice".to_owned(),
            column: "invented_note".to_owned(),
        },
        CoverageFinding::ColumnNotInSchema {
            table: "user".to_owned(),
            column: "title".to_owned(),
        },
    ] {
        assert!(found.contains(&expected), "{expected:?} among {found:#?}");
    }
    assert_eq!(found.len(), 3, "{found:#?}");
}

#[tokio::test]
async fn a_check_list_that_differs_from_the_vocabulary_fails_coverage() {
    let (_tmp, pool) = scratch().await;
    let mut conn = pool.acquire().await.expect("connection");
    // The same table with one vocabulary value gone and a constraint
    // added to a column the specification gives none.
    for sql in [
        "DROP TABLE phase_transition",
        "CREATE TABLE phase_transition (
            id INTEGER PRIMARY KEY,
            program_version_id INTEGER NOT NULL REFERENCES program_version (id),
            from_phase_id INTEGER NOT NULL,
            to_phase_id INTEGER NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('advance', 'remediation', 'restart')),
            FOREIGN KEY (from_phase_id, program_version_id)
                REFERENCES phase (id, program_version_id),
            FOREIGN KEY (to_phase_id, program_version_id)
                REFERENCES phase (id, program_version_id)
        ) STRICT",
        "DROP TABLE notice",
        "CREATE TABLE notice (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES user (id),
            kind TEXT NOT NULL CHECK (kind IN ('backup_failed')),
            message TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            read_at INTEGER
        ) STRICT",
    ] {
        sqlx::query(sql).execute(&mut *conn).await.expect(sql);
    }
    let found = coverage_findings(&mut conn).await.expect("coverage");
    assert_eq!(
        found,
        vec![
            CoverageFinding::Vocabulary {
                table: "notice".to_owned(),
                column: "kind".to_owned(),
                stored: vec!["backup_failed".to_owned()],
                spec: Vec::new(),
            },
            CoverageFinding::Vocabulary {
                table: "phase_transition".to_owned(),
                column: "kind".to_owned(),
                stored: vec![
                    "advance".to_owned(),
                    "remediation".to_owned(),
                    "restart".to_owned()
                ],
                spec: vec![
                    "advance".to_owned(),
                    "remediation".to_owned(),
                    "restart".to_owned(),
                    "skip".to_owned()
                ],
            },
        ]
    );
}

/// Coverage of a scratch database after `edits`: each edit recreates one
/// stored table with `from` replaced by `to` in its SQL (an empty `from`
/// drops the table), on one connection with foreign keys off so a parent
/// can be recreated under its children.
async fn after(edits: &[(&str, &str, &str)]) -> Vec<CoverageFinding> {
    let (_tmp, pool) = scratch().await;
    let mut conn = pool.acquire().await.expect("connection");
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .expect("foreign keys off");
    for (table, from, to) in edits {
        let sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name = ?1")
            .bind(table)
            .fetch_one(&mut *conn)
            .await
            .expect("table sql");
        sqlx::query(&format!("DROP TABLE \"{table}\""))
            .execute(&mut *conn)
            .await
            .expect("drop");
        if from.is_empty() {
            continue;
        }
        assert!(sql.contains(from), "the control edits real SQL: {from}");
        sqlx::query(&sql.replacen(from, to, 1))
            .execute(&mut *conn)
            .await
            .expect("recreate");
    }
    coverage_findings(&mut conn).await.expect("coverage")
}

fn text(value: &str) -> String {
    value.to_owned()
}

/// Every stored foreign key is compared, so a second key on a column
/// cannot hide behind the first (before #98's review fix, this composite
/// key overwrote the column's plain one and coverage came back empty).
#[tokio::test]
async fn a_second_foreign_key_on_a_column_is_reported() {
    let found = after(&[(
        "evaluation_record",
        "UNIQUE (id, program_version_id)",
        "FOREIGN KEY (enrollment_id, program_version_id)
             REFERENCES enrollment (id, program_version_id),
         UNIQUE (id, program_version_id)",
    )])
    .await;
    assert_eq!(
        found,
        vec![CoverageFinding::ReferenceNotInSpec {
            table: text("evaluation_record"),
            column: text("enrollment_id"),
            target: text("enrollment"),
            pinned: true,
        }]
    );
}

/// One table recreated with `from` replaced by `to`.
type Edit<'a> = (&'a str, &'a str, &'a str);
/// What a control does, its edits, and the findings it must produce.
type Case<'a> = (&'a str, Vec<Edit<'a>>, Vec<CoverageFinding>);

/// Each kind of schema drift names its finding.
#[tokio::test]
async fn every_kind_of_schema_drift_is_reported() {
    let cases: Vec<Case<'_>> = vec![
        (
            "column type",
            vec![("notice", "read_at INTEGER", "read_at TEXT")],
            vec![CoverageFinding::ColumnType {
                table: text("notice"),
                column: text("read_at"),
                stored: text("TEXT"),
                spec: "integer",
            }],
        ),
        (
            "nullability",
            vec![("notice", "read_at INTEGER", "read_at INTEGER NOT NULL")],
            vec![CoverageFinding::ColumnNullability {
                table: text("notice"),
                column: text("read_at"),
                stored_nullable: false,
            }],
        ),
        (
            "key order",
            vec![(
                "capability_grant",
                "PRIMARY KEY (user_id, capability)",
                "PRIMARY KEY (capability, user_id)",
            )],
            vec![CoverageFinding::Key {
                table: text("capability_grant"),
                stored: vec![text("capability"), text("user_id")],
                spec: vec![text("user_id"), text("capability")],
            }],
        ),
        (
            "new reference",
            vec![(
                "notice",
                "read_at INTEGER",
                "read_at INTEGER REFERENCES user (id)",
            )],
            vec![CoverageFinding::ReferenceNotInSpec {
                table: text("notice"),
                column: text("read_at"),
                target: text("user"),
                pinned: false,
            }],
        ),
        (
            "pin removed",
            vec![(
                "task",
                "FOREIGN KEY (competency_id, program_version_id)
        REFERENCES competency (id, program_version_id)",
                "FOREIGN KEY (competency_id) REFERENCES competency (id)",
            )],
            vec![
                CoverageFinding::ReferenceNotInSpec {
                    table: text("task"),
                    column: text("competency_id"),
                    target: text("competency"),
                    pinned: false,
                },
                CoverageFinding::ReferenceNotInSchema {
                    table: text("task"),
                    column: text("competency_id"),
                    target: "competency",
                    pinned: true,
                },
            ],
        ),
        (
            "spec table dropped",
            vec![("hold_release", "", "")],
            vec![CoverageFinding::TableNotInSchema {
                table: text("hold_release"),
            }],
        ),
        (
            "lowercase, newline-split CHECK list",
            vec![(
                "notice",
                "kind TEXT NOT NULL,",
                "kind TEXT NOT NULL CHECK (kind\n    in ('backup_failed')),",
            )],
            vec![CoverageFinding::Vocabulary {
                table: text("notice"),
                column: text("kind"),
                stored: vec![text("backup_failed")],
                spec: Vec::new(),
            }],
        ),
    ];
    for (what, edits, expected) in cases {
        assert_eq!(after(&edits).await, expected, "{what}");
    }
}

/// A dotted exclusion or a unit column must be a stored column.
#[tokio::test]
async fn an_excluded_column_the_schema_lost_is_reported() {
    let (_tmp, pool) = scratch().await;
    for sql in [
        "ALTER TABLE user DROP COLUMN password_hash",
        "ALTER TABLE evaluation_version DROP COLUMN canonical_bytes",
    ] {
        sqlx::query(sql).execute(&pool).await.expect(sql);
    }
    assert_eq!(
        findings(&pool).await,
        vec![
            CoverageFinding::ExcludedColumnNotInSchema {
                name: text("user.password_hash"),
            },
            CoverageFinding::ExcludedColumnNotInSchema {
                name: text("evaluation_version.canonical_bytes"),
            },
        ]
    );
}

/// No name is both carried and excluded or carried as units.
#[test]
fn a_name_both_carried_and_excluded_is_reported() {
    assert_eq!(
        specification_findings(
            &table_spec::TABLES,
            &table_spec::EXCLUSIONS,
            &table_spec::UNIT_COLUMNS
        ),
        Vec::new()
    );
    let mut exclusions = table_spec::EXCLUSIONS.to_vec();
    for name in ["notice", "user.username"] {
        exclusions.push(Exclusion {
            name,
            reason: ExclusionReason::Credential,
        });
    }
    let mut units = table_spec::UNIT_COLUMNS.to_vec();
    units.push(("evaluation_version", "content_hash"));
    assert_eq!(
        specification_findings(&table_spec::TABLES, &exclusions, &units),
        vec![
            CoverageFinding::CarriedAndExcluded {
                name: text("notice")
            },
            CoverageFinding::CarriedAndExcluded {
                name: text("user.username"),
            },
            CoverageFinding::CarriedAndExcluded {
                name: text("evaluation_version.content_hash"),
            },
        ]
    );
}

/// `IN` lists are read in any case, after any whitespace, with or without
/// a space before the parenthesis; `NOT IN` and words containing `in` are
/// not lists.
#[test]
fn check_lists_are_read_however_they_are_spelled() {
    for sql in [
        "kind TEXT CHECK (kind IN ('a', 'b'))",
        "kind TEXT CHECK (kind\nIN ('a', 'b'))",
        "kind TEXT CHECK (kind\tin('a', 'b'))",
        "kind TEXT CHECK (kind in (\n'a', 'b'))",
    ] {
        let lists = table_spec::in_lists(sql);
        assert_eq!(lists.len(), 1, "{sql:?}: {lists:?}");
        assert_eq!(lists["kind"], vec![text("a"), text("b")], "{sql:?}");
    }
    for sql in [
        "kind TEXT CHECK (kind NOT IN ('a'))",
        "kind TEXT CHECK (kind = 'in') -- kind IN ('a')",
        "id INTEGER, inside INTEGER, CHECK (main (1))",
    ] {
        assert!(table_spec::in_lists(sql).is_empty(), "{sql:?}");
    }
}
