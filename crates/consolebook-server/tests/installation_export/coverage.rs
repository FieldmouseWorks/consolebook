//! The format's table and column lists are enforced, not just written
//! down: the coverage guard compares the migrated schema with the table
//! specification both ways, and the format document's column tables,
//! agreements, and module vocabularies are compared with the same
//! specification.

use std::collections::BTreeMap;

use consolebook_server::installation_export::spec::{
    CoverageFinding, Vocabulary, coverage_findings,
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

// ------------------------------------------------------- document sync

const FORMAT_DOC: &str = include_str!("../../../../docs/formats/installation-export.md");

/// The section of `doc` from `start` to the next heading that `end`
/// begins.
fn section<'a>(doc: &'a str, start: &str, end: &str) -> &'a str {
    let from = doc.find(start).unwrap_or_else(|| panic!("no {start}"));
    let rest = &doc[from + start.len()..];
    let to = rest.find(end).unwrap_or(rest.len());
    &rest[..to]
}

fn cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn unquote(cell: &str) -> String {
    cell.trim_matches('`').to_owned()
}

/// Every backticked token in `text`.
fn ticked(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// A column's rule as the format document writes it.
fn rule(table: &spec::Table, column: &spec::Column) -> String {
    let mut parts = Vec::new();
    if let Some(position) = table.key.iter().position(|key| *key == column.name) {
        parts.push(if table.key.len() == 1 {
            "key".to_owned()
        } else {
            format!("key ({})", position + 1)
        });
    }
    if let Some(target) = column.references {
        parts.push(format!("→ `{target}`"));
    }
    match column.vocabulary {
        Some(Vocabulary::OneOf(values)) => parts.push(format!(
            "one of {}",
            values
                .iter()
                .map(|value| format!("`{value}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Some(vocabulary) => parts.push(format!(
            "{} (see Vocabularies)",
            vocabulary.module_name().expect("module vocabulary")
        )),
        None => {}
    }
    if let Some(form) = column.form {
        parts.push(form.as_str().to_owned());
    }
    parts.join("; ")
}

/// Every difference between the format document `doc` and the table
/// specification: the column tables (table, column, type, nullability,
/// rule, in stored order), the agreements table, and the four module
/// vocabularies.
fn doc_differences(doc: &str) -> Vec<String> {
    let mut differences = Vec::new();

    // Column tables.
    let mut documented: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for line in section(doc, "### Columns", "### References").lines() {
        let row = cells(line);
        if row.len() != 5 || !row[0].starts_with('`') {
            continue;
        }
        documented.entry(unquote(&row[0])).or_default().push(vec![
            unquote(&row[1]),
            row[2].clone(),
            row[3].clone(),
            row[4].clone(),
        ]);
    }
    let mut specified: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for table in &spec::TABLES {
        let rows = table
            .columns
            .iter()
            .map(|column| {
                vec![
                    column.name.to_owned(),
                    column.ty.as_str().to_owned(),
                    if column.nullable { "yes" } else { "no" }.to_owned(),
                    rule(table, column),
                ]
            })
            .collect();
        specified.insert(table.name.to_owned(), rows);
    }
    for (table, rows) in &specified {
        match documented.get(table) {
            None => differences.push(format!("{table}: not documented")),
            Some(documented) if documented != rows => differences.push(format!(
                "{table}: documented {documented:?}, specified {rows:?}"
            )),
            Some(_) => {}
        }
    }
    for table in documented.keys() {
        if !specified.contains_key(table) {
            differences.push(format!("{table}: documented, not specified"));
        }
    }

    // Agreements.
    let mut documented: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in section(doc, "### Agreements", "### Vocabularies").lines() {
        let row = cells(line);
        if row.len() != 2 || !row[0].starts_with('`') {
            continue;
        }
        let columns = ticked(&row[1])
            .into_iter()
            .filter(|name| name != "null")
            .collect();
        documented.insert(unquote(&row[0]), columns);
    }
    let specified: BTreeMap<String, Vec<String>> = spec::TABLES
        .iter()
        .filter_map(|table| {
            let pinned: Vec<String> = table
                .columns
                .iter()
                .filter(|column| column.agrees)
                .map(|column| column.name.to_owned())
                .collect();
            (!pinned.is_empty()).then(|| (table.name.to_owned(), pinned))
        })
        .collect();
    if documented != specified {
        differences.push(format!(
            "agreements: documented {documented:?}, specified {specified:?}"
        ));
    }

    // Module vocabularies: each bullet's backticked values, after the
    // owning file's name.
    let vocabularies = section(doc, "### Vocabularies", "## What the export does not carry");
    for vocabulary in [
        Vocabulary::Capability,
        Vocabulary::NoticeKind,
        Vocabulary::AuditEventKind,
        Vocabulary::AuditSubjectKind,
    ] {
        let name = vocabulary.module_name().expect("module vocabulary");
        let marker = format!("- **{name}** (");
        let Some(from) = vocabularies.find(&marker) else {
            differences.push(format!("{name}: not documented"));
            continue;
        };
        let rest = &vocabularies[from + marker.len()..];
        let bullet = &rest[..rest.find("\n- ").unwrap_or(rest.len())];
        let mut documented: Vec<String> = ticked(bullet).into_iter().skip(1).collect();
        documented.sort();
        let mut specified: Vec<String> =
            vocabulary.values().into_iter().map(str::to_owned).collect();
        specified.sort();
        if documented != specified {
            differences.push(format!(
                "{name}: documented {documented:?}, specified {specified:?}"
            ));
        }
    }
    differences
}

#[test]
fn the_format_document_states_the_specification() {
    assert_eq!(doc_differences(FORMAT_DOC), Vec::<String>::new());
}

#[test]
fn a_drifted_format_document_is_reported() {
    let edits = [
        // A type.
        (
            "| `user` | `title` | text | no |  |",
            "| `user` | `title` | integer | no |  |",
            "user:",
        ),
        // A reference target.
        (
            "| `notice` | `user_id` | integer | no | → `user` |",
            "| `notice` | `user_id` | integer | no | → `enrollment` |",
            "notice:",
        ),
        // A CHECK vocabulary value.
        (
            "`litigation`, `anticipated_litigation`",
            "`litigation`",
            "record_hold:",
        ),
        // An agreement.
        (
            "| `task` | `competency_id` |\n",
            "| `task` | `program_version_id` |\n",
            "agreements:",
        ),
        // A module vocabulary value.
        (
            "and `installation_exported` (see Authorization)",
            "(see Authorization)",
            "audit event kind:",
        ),
        // A column dropped from the document.
        (
            "| `notice` | `read_at` | integer | yes |  |\n",
            "",
            "notice:",
        ),
    ];
    for (from, to, reported) in edits {
        assert!(
            FORMAT_DOC.contains(from),
            "the control edits real text: {from}"
        );
        let drifted = FORMAT_DOC.replacen(from, to, 1);
        let differences = doc_differences(&drifted);
        assert!(
            differences
                .iter()
                .any(|difference| difference.starts_with(reported)),
            "{from:?} → {to:?}: {differences:#?}"
        );
    }
}
