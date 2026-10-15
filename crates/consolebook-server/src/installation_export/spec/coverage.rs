//! The coverage guard: the migrated schema compared with the table
//! specification both ways (docs/formats/installation-export.md,
//! Coverage and format versions).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use sqlx::{Row, SqliteConnection};

use super::{ColumnType, EXCLUSIONS, TABLES, Table, UNIT_COLUMNS, Vocabulary, exclusion, table};

/// One difference between the migrated schema and the specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageFinding {
    /// A stored table the specification neither carries nor excludes.
    TableNotInSpec { table: String },
    /// A specified table (carried or excluded) the schema does not have.
    TableNotInSchema { table: String },
    /// A stored column of a carried table that is neither carried,
    /// excluded, nor carried as units.
    ColumnNotInSpec { table: String, column: String },
    /// A specified column the stored table does not have.
    ColumnNotInSchema { table: String, column: String },
    ColumnType {
        table: String,
        column: String,
        stored: String,
        spec: &'static str,
    },
    ColumnNullability {
        table: String,
        column: String,
        stored_nullable: bool,
    },
    /// The stored primary key is not the specified key.
    Key {
        table: String,
        stored: Vec<String>,
        spec: Vec<String>,
    },
    /// The column's stored `CHECK ... IN (...)` values differ from the
    /// specified vocabulary (an absent list is empty).
    Vocabulary {
        table: String,
        column: String,
        stored: Vec<String>,
        spec: Vec<String>,
    },
    /// The column's stored foreign key names a different table than the
    /// specified reference (or one exists where none is specified, or the
    /// reverse).
    Reference {
        table: String,
        column: String,
        stored: Option<String>,
        spec: Option<&'static str>,
    },
    /// The stored composite key pins `program_version_id` where the
    /// specification does not, or the reverse.
    Agreement {
        table: String,
        column: String,
        stored: bool,
    },
}

/// One stored column, as `pragma table_info` reports it.
struct StoredColumn {
    name: String,
    ty: String,
    not_null: bool,
    pk: i64,
}

/// Compares the migrated schema on `conn` with the specification, both
/// directions: every stored table and column (but SQLite's own `sqlite_`
/// tables and the migration bookkeeping table) is carried, excluded, or
/// carried as units, and every specified table and column is stored, with
/// the same type, nullability, key, references, agreements, and `CHECK`
/// vocabularies.
pub async fn coverage_findings(conn: &mut SqliteConnection) -> Result<Vec<CoverageFinding>> {
    let stored: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master
         WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
           AND name != '_sqlx_migrations'
         ORDER BY name",
    )
    .fetch_all(&mut *conn)
    .await
    .context("listing stored tables")?;
    let mut findings = Vec::new();
    let stored_names: BTreeSet<&str> = stored.iter().map(|(name, _)| name.as_str()).collect();
    for name in &stored_names {
        if table(name).is_none() && exclusion(name).is_none() {
            findings.push(CoverageFinding::TableNotInSpec {
                table: (*name).to_owned(),
            });
        }
    }
    let specified = TABLES.iter().map(|table| table.name).chain(
        EXCLUSIONS
            .iter()
            .filter(|exclusion| !exclusion.name.contains('.'))
            .map(|exclusion| exclusion.name),
    );
    for name in specified {
        if !stored_names.contains(name) {
            findings.push(CoverageFinding::TableNotInSchema {
                table: name.to_owned(),
            });
        }
    }
    for (name, sql) in &stored {
        let Some(spec) = table(name) else { continue };
        let columns = stored_columns(conn, name).await?;
        let references = stored_references(conn, name).await?;
        findings.extend(compare_table(spec, sql, &columns, &references));
    }
    Ok(findings)
}

async fn stored_columns(conn: &mut SqliteConnection, table: &str) -> Result<Vec<StoredColumn>> {
    let rows = sqlx::query(r#"SELECT name, type, "notnull", pk FROM pragma_table_info(?1)"#)
        .bind(table)
        .fetch_all(&mut *conn)
        .await
        .with_context(|| format!("reading the columns of {table}"))?;
    Ok(rows
        .iter()
        .map(|row| StoredColumn {
            name: row.get("name"),
            ty: row.get("type"),
            not_null: row.get::<i64, _>("notnull") != 0,
            pk: row.get("pk"),
        })
        .collect())
}

/// What the stored foreign keys say about each column: the table its
/// value names by `id`, and whether the same key pins
/// `program_version_id` to the referenced row's. A foreign-key pair of
/// any other shape is reported under the column as a reference to
/// `table(column)`, which no specification entry matches.
async fn stored_references(
    conn: &mut SqliteConnection,
    table: &str,
) -> Result<BTreeMap<String, (String, bool)>> {
    let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
        r#"SELECT id, "table", "from", "to" FROM pragma_foreign_key_list(?1) ORDER BY id, seq"#,
    )
    .bind(table)
    .fetch_all(&mut *conn)
    .await
    .with_context(|| format!("reading the foreign keys of {table}"))?;
    let mut keys: BTreeMap<i64, Vec<(String, String, String)>> = BTreeMap::new();
    for (id, target, from, to) in rows {
        keys.entry(id)
            .or_default()
            .push((target, from, to.unwrap_or_default()));
    }
    let mut references = BTreeMap::new();
    for pairs in keys.values() {
        let pinned = pairs
            .iter()
            .any(|(_, from, to)| from == "program_version_id" && to == "program_version_id");
        for (target, from, to) in pairs {
            if from == "program_version_id" && to == "program_version_id" && pairs.len() > 1 {
                continue;
            }
            let named = if to == "id" {
                target.clone()
            } else {
                format!("{target}({to})")
            };
            references.insert(from.clone(), (named, pinned));
        }
    }
    // A `program_version_id` that only composite keys pin has no foreign
    // key of its own, but those keys make it name a row of the referenced
    // configuration, whose own `program_version_id` names a program
    // version: the value resolves into `program_version` all the same.
    let pins = keys.values().any(|pairs| {
        pairs.len() > 1
            && pairs
                .iter()
                .any(|(_, from, to)| from == "program_version_id" && to == "program_version_id")
    });
    if pins && !references.contains_key("program_version_id") {
        references.insert(
            "program_version_id".to_owned(),
            ("program_version".to_owned(), false),
        );
    }
    Ok(references)
}

fn compare_table(
    spec: &Table,
    sql: &str,
    columns: &[StoredColumn],
    references: &BTreeMap<String, (String, bool)>,
) -> Vec<CoverageFinding> {
    let mut findings = Vec::new();
    let table = spec.name.to_owned();
    let lists = in_lists(sql);
    for stored in columns {
        let qualified = format!("{}.{}", spec.name, stored.name);
        let travels_as_unit = UNIT_COLUMNS.contains(&(spec.name, stored.name.as_str()));
        let Some(column) = spec.column(&stored.name) else {
            if exclusion(&qualified).is_none() && !travels_as_unit {
                findings.push(CoverageFinding::ColumnNotInSpec {
                    table: table.clone(),
                    column: stored.name.clone(),
                });
            }
            continue;
        };
        if stored.ty != column.ty.stored_type() {
            findings.push(CoverageFinding::ColumnType {
                table: table.clone(),
                column: stored.name.clone(),
                stored: stored.ty.clone(),
                spec: column.ty.as_str(),
            });
        }
        // An INTEGER PRIMARY KEY is the rowid and is never null, whatever
        // `notnull` says.
        let stored_nullable = !stored.not_null && stored.pk == 0;
        if stored_nullable != column.nullable {
            findings.push(CoverageFinding::ColumnNullability {
                table: table.clone(),
                column: stored.name.clone(),
                stored_nullable,
            });
        }
        let stored_list: Vec<String> = lists.get(&stored.name).cloned().unwrap_or_default();
        let spec_list: Vec<String> = match (column.ty, column.vocabulary) {
            (ColumnType::Flag, _) if !stored_list.is_empty() => {
                vec!["0".to_owned(), "1".to_owned()]
            }
            (_, Some(Vocabulary::OneOf(values))) => {
                let mut values: Vec<String> = values.iter().map(|v| (*v).to_owned()).collect();
                values.sort();
                values
            }
            _ => Vec::new(),
        };
        if stored_list != spec_list {
            findings.push(CoverageFinding::Vocabulary {
                table: table.clone(),
                column: stored.name.clone(),
                stored: stored_list,
                spec: spec_list,
            });
        }
        let (stored_reference, stored_pinned) = match references.get(&stored.name) {
            Some((target, pinned)) => (Some(target.clone()), *pinned),
            None => (None, false),
        };
        if stored_reference.as_deref() != column.references {
            findings.push(CoverageFinding::Reference {
                table: table.clone(),
                column: stored.name.clone(),
                stored: stored_reference,
                spec: column.references,
            });
        } else if stored_pinned != column.agrees {
            findings.push(CoverageFinding::Agreement {
                table: table.clone(),
                column: stored.name.clone(),
                stored: stored_pinned,
            });
        }
    }
    for column in spec.columns {
        if !columns.iter().any(|stored| stored.name == column.name) {
            findings.push(CoverageFinding::ColumnNotInSchema {
                table: table.clone(),
                column: column.name.to_owned(),
            });
        }
    }
    let mut keyed: Vec<&StoredColumn> = columns.iter().filter(|stored| stored.pk > 0).collect();
    keyed.sort_by_key(|stored| stored.pk);
    let stored_key: Vec<String> = keyed.iter().map(|stored| stored.name.clone()).collect();
    let spec_key: Vec<String> = spec.key.iter().map(|name| (*name).to_owned()).collect();
    if stored_key != spec_key {
        findings.push(CoverageFinding::Key {
            table,
            stored: stored_key,
            spec: spec_key,
        });
    }
    findings
}

/// Every `column IN (...)` literal list in a table's SQL, by column,
/// merged and sorted: a column constrained in more than one place (a
/// column check and a table check naming a subset) has the union. String
/// literals lose their quotes; integers stay as written. Comments are
/// skipped.
#[must_use]
pub fn in_lists(sql: &str) -> BTreeMap<String, Vec<String>> {
    let code: String = sql
        .lines()
        .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = code.as_bytes();
    let mut lists: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut at = 0;
    while let Some(found) = code[at..].find(" IN") {
        let start = at + found;
        at = start + 3;
        // The keyword is followed by optional space and an opening paren.
        let rest = code[at..].trim_start();
        if !rest.starts_with('(') || bytes.get(at).is_some_and(|b| is_word(*b)) {
            continue;
        }
        // The identifier immediately before the keyword.
        let before = code[..start].trim_end();
        let ident_start = before
            .bytes()
            .rposition(|byte| !is_word(byte))
            .map_or(0, |index| index + 1);
        let column = &before[ident_start..];
        if column.is_empty() || column.eq_ignore_ascii_case("NOT") {
            continue;
        }
        let open = code.len() - rest.len();
        let Some(close) = code[open..].find(')') else {
            break;
        };
        let values = code[open + 1..open + close]
            .split(',')
            .map(|value| value.trim().trim_matches('\'').to_owned());
        lists.entry(column.to_owned()).or_default().extend(values);
        at = open + close;
    }
    lists
        .into_iter()
        .map(|(column, values)| (column, values.into_iter().collect()))
        .collect()
}
