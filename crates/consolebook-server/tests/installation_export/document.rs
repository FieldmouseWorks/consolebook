//! The format document and the format version move with the table
//! specification: the document's column tables, agreements, exclusions,
//! counts, and module vocabularies are compared with it, and its
//! fingerprint is pinned per `format_version`.

use std::collections::BTreeMap;

use consolebook_server::installation_export::spec::Vocabulary;

use super::*;

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

    differences.extend(vocabulary_differences(doc));
    differences.extend(exclusion_differences(doc));
    differences
}

/// The four module vocabularies the document lists, against their
/// owning modules' values.
fn vocabulary_differences(doc: &str) -> Vec<String> {
    let mut differences = Vec::new();
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

/// The exclusions the document states — the manifest example's
/// `excluded` list with reasons, and the credentials bullet's names — and
/// the counts the ledger section states, against the specification.
fn exclusion_differences(doc: &str) -> Vec<String> {
    let mut differences = Vec::new();
    // The exclusions: the manifest example's `excluded` list (names and
    // reasons) and the credentials bullet's names.
    let mut specified: Vec<(String, String)> = spec::EXCLUSIONS
        .iter()
        .map(|e| (e.name.to_owned(), e.reason.as_str().to_owned()))
        .collect();
    specified.sort();
    let example = section(doc, "## Manifest (`manifest.json`)", "| Member |");
    let excluded = section(example, "\"excluded\": [", "]");
    let mut documented: Vec<(String, String)> = excluded
        .lines()
        .filter_map(|line| {
            let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
            match quoted.as_slice() {
                ["name", name, "reason", reason] => {
                    Some(((*name).to_owned(), (*reason).to_owned()))
                }
                _ => None,
            }
        })
        .collect();
    documented.sort();
    if documented != specified {
        differences.push(format!(
            "excluded example: documented {documented:?}, specified {specified:?}"
        ));
    }
    let credentials = section(doc, "- **Credentials:**", "\n- ");
    let mut documented = ticked(credentials);
    documented.sort();
    let names: Vec<String> = specified.iter().map(|(name, _)| name.clone()).collect();
    if documented != names {
        differences.push(format!(
            "credentials: documented {documented:?}, specified {names:?}"
        ));
    }

    // The counts the ledger section states.
    let counts = format!(
        "Version 1 has {} carried tables and\n{} exclusions",
        spec::TABLES.len(),
        spec::EXCLUSIONS.len()
    );
    if !doc.contains(&counts) {
        differences.push(format!("counts: the document does not say {counts:?}"));
    }
    differences
}

/// The table specification's fingerprint for each format version, as
/// recorded when that version was defined. A changed specification is a
/// new `format_version` with its own recorded fingerprint, appended here:
/// never an edit to an existing entry, because archives of that version
/// already exist and keep their meaning (ADR 0028). The fingerprint
/// pins spec-held structure; vocabulary members grow within a version and
/// are held by the vocabulary floor (`floor.rs`) instead.
const RECORDED_FINGERPRINTS: &[(i64, &str)] = &[(
    1,
    "5d4b236558b3769d42629ecd6ed5e8608027fbce68e0aa09deb245cbbc36cde5",
)];

#[test]
fn the_specification_is_pinned_to_its_format_version() {
    let recorded = RECORDED_FINGERPRINTS
        .iter()
        .find(|(version, _)| *version == INSTALLATION_FORMAT_VERSION)
        .unwrap_or_else(|| {
            panic!(
                "format_version {INSTALLATION_FORMAT_VERSION} has no recorded fingerprint: \
                 append ({INSTALLATION_FORMAT_VERSION}, \"{}\")",
                spec::fingerprint()
            )
        });
    assert_eq!(
        spec::fingerprint(),
        recorded.1,
        "the table specification changed under format_version {INSTALLATION_FORMAT_VERSION}. \
         A changed specification is a new format_version: bump INSTALLATION_FORMAT_VERSION \
         and append its fingerprint as a new entry; never edit an existing entry \
         (docs/formats/installation-export.md, Coverage and format versions)"
    );
}

/// Any change to what the specification says changes its fingerprint.
#[test]
fn a_changed_specification_changes_its_fingerprint() {
    fn digest(
        tables: &[spec::Table],
        exclusions: &[spec::Exclusion],
        units: &[(&str, &str)],
    ) -> String {
        canonical::content_hash_hex(spec::describe(tables, exclusions, units).as_bytes())
    }
    fn with_column(edit: fn(&mut spec::Column)) -> String {
        let mut tables = spec::TABLES.to_vec();
        let mut columns = tables[0].columns.to_vec();
        edit(&mut columns[1]);
        tables[0].columns = Box::leak(columns.into_boxed_slice());
        digest(&tables, &spec::EXCLUSIONS, &spec::UNIT_COLUMNS)
    }
    let base = digest(&spec::TABLES, &spec::EXCLUSIONS, &spec::UNIT_COLUMNS);
    assert_eq!(base, spec::fingerprint());
    let mut rekeyed = spec::TABLES.to_vec();
    rekeyed[0].key = &["user_id"];
    let mutations = [
        ("nullability", with_column(|c| c.nullable = !c.nullable)),
        ("type", with_column(|c| c.ty = spec::ColumnType::Text)),
        ("reference", with_column(|c| c.references = Some("user"))),
        ("agreement", with_column(|c| c.agrees = !c.agrees)),
        (
            "vocabulary",
            with_column(|c| c.vocabulary = Some(Vocabulary::OneOf(&["x"]))),
        ),
        (
            "text form",
            with_column(|c| c.form = Some(spec::TextForm::LowercaseHex64)),
        ),
        (
            "key",
            digest(&rekeyed, &spec::EXCLUSIONS, &spec::UNIT_COLUMNS),
        ),
        (
            "exclusion",
            digest(&spec::TABLES, &spec::EXCLUSIONS[1..], &spec::UNIT_COLUMNS),
        ),
        ("unit column", digest(&spec::TABLES, &spec::EXCLUSIONS, &[])),
    ];
    for (what, mutated) in mutations {
        assert_ne!(mutated, base, "{what} left the fingerprint unchanged");
    }
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
        // An exclusion in the manifest example.
        (
            "{ \"name\": \"setup_code\", \"reason\": \"credential\" },\n",
            "",
            "excluded example:",
        ),
        // An exclusion in the credentials bullet.
        ("and `setup_code` tables", "tables", "credentials:"),
        // A count.
        ("43 carried tables", "44 carried tables", "counts:"),
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
