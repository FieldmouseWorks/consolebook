//! Program content persistence and transaction-owned inserts.

use super::content::{
    AnchorDef, CitationDef, CompetencyDef, FormCompetencyDef, FormDef, ModifierDef, NarrativeDef,
    PhaseDef, PolicyDef, RecordType, ScaleDef, ScaleKind, TaskDef, TransitionDef, TransitionKind,
    VersionContent,
};
use crate::audit::{self, EventKind, Subject};
use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use sqlx::{Row, SqliteConnection, SqlitePool};
use time::OffsetDateTime;

pub(crate) async fn insert_program(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    actor_user_id: i64,
) -> Result<i64> {
    let result =
        sqlx::query("INSERT INTO program (name, created_at, created_by) VALUES (?1, ?2, ?3)")
            .bind(name)
            .bind(OffsetDateTime::now_utc().unix_timestamp())
            .bind(actor_user_id)
            .execute(&mut **tx)
            .await
            .context("creating program")?;
    let program_id = result.last_insert_rowid();
    audit::record_for_subject(
        &mut **tx,
        EventKind::ProgramCreated,
        Some(actor_user_id),
        None,
        Subject::Program(program_id),
    )
    .await?;
    Ok(program_id)
}

/// Inserts a draft version row plus its content and records the lifecycle
/// audit event. Callers have already validated the content.
pub(crate) async fn insert_version(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    program_id: i64,
    content: &VersionContent,
    actor_user_id: i64,
    kind: EventKind,
) -> Result<i64> {
    let next_number: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version_number), 0) + 1 FROM program_version WHERE program_id = ?1",
    )
    .bind(program_id)
    .fetch_one(&mut **tx)
    .await
    .context("numbering version")?;
    let result = sqlx::query(
        "INSERT INTO program_version
             (program_id, version_number, label, name, description, created_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(program_id)
    .bind(next_number)
    .bind(&content.label)
    .bind(&content.name)
    .bind(&content.description)
    .bind(OffsetDateTime::now_utc().unix_timestamp())
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await
    .context("creating program version")?;
    let version_id = result.last_insert_rowid();
    insert_content(tx, version_id, content).await?;
    audit::record_for_subject(
        &mut **tx,
        kind,
        Some(actor_user_id),
        None,
        Subject::ProgramVersion(version_id),
    )
    .await?;
    Ok(version_id)
}

// ---- content writes

async fn insert_citation(
    conn: &mut SqliteConnection,
    version_id: i64,
    competency_id: Option<i64>,
    task_id: Option<i64>,
    citation: &CitationDef,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO standards_citation
             (program_version_id, competency_id, task_id, body, edition, clause, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(version_id)
    .bind(competency_id)
    .bind(task_id)
    .bind(&citation.body)
    .bind(&citation.edition)
    .bind(&citation.clause)
    .bind(&citation.note)
    .execute(conn)
    .await
    .context("inserting standards citation")?;
    Ok(())
}

async fn insert_phases(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
) -> Result<()> {
    let mut phase_ids: HashMap<&str, i64> = HashMap::new();
    for phase in &content.phases {
        let result = sqlx::query(
            "INSERT INTO phase (program_version_id, name, description, presentation_number)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(version_id)
        .bind(&phase.name)
        .bind(&phase.description)
        .bind(phase.presentation_number)
        .execute(&mut *conn)
        .await
        .context("inserting phase")?;
        phase_ids.insert(phase.name.as_str(), result.last_insert_rowid());
    }
    for transition in &content.phase_transitions {
        sqlx::query(
            "INSERT INTO phase_transition
                 (program_version_id, from_phase_id, to_phase_id, kind)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(version_id)
        .bind(phase_ids[transition.from_phase.as_str()])
        .bind(phase_ids[transition.to_phase.as_str()])
        .bind(transition.kind.as_str())
        .execute(&mut *conn)
        .await
        .context("inserting phase transition")?;
    }
    Ok(())
}

async fn insert_competencies(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
) -> Result<HashMap<String, i64>> {
    let mut competency_ids: HashMap<String, i64> = HashMap::new();
    for (order, competency) in (0_i64..).zip(&content.competencies) {
        let result = sqlx::query(
            "INSERT INTO competency (program_version_id, category, name, description, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(version_id)
        .bind(&competency.category)
        .bind(&competency.name)
        .bind(&competency.description)
        .bind(order)
        .execute(&mut *conn)
        .await
        .context("inserting competency")?;
        let competency_id = result.last_insert_rowid();
        competency_ids.insert(competency.name.clone(), competency_id);
        for (task_order, task) in (0_i64..).zip(&competency.tasks) {
            let inserted = sqlx::query(
                "INSERT INTO task (program_version_id, competency_id, prompt, sort_order)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(version_id)
            .bind(competency_id)
            .bind(&task.prompt)
            .bind(task_order)
            .execute(&mut *conn)
            .await
            .context("inserting task")?;
            let task_id = inserted.last_insert_rowid();
            for citation in &task.citations {
                insert_citation(&mut *conn, version_id, None, Some(task_id), citation).await?;
            }
        }
        for citation in &competency.citations {
            insert_citation(&mut *conn, version_id, Some(competency_id), None, citation).await?;
        }
    }
    Ok(competency_ids)
}

async fn insert_scales(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
) -> Result<HashMap<String, i64>> {
    let mut scale_ids: HashMap<String, i64> = HashMap::new();
    for scale in &content.rating_scales {
        let result = sqlx::query(
            "INSERT INTO rating_scale (program_version_id, name, kind, min_value, max_value)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(version_id)
        .bind(&scale.name)
        .bind(scale.kind.as_str())
        .bind(scale.min_value)
        .bind(scale.max_value)
        .execute(&mut *conn)
        .await
        .context("inserting rating scale")?;
        let scale_id = result.last_insert_rowid();
        scale_ids.insert(scale.name.clone(), scale_id);
        for anchor in &scale.anchors {
            sqlx::query(
                "INSERT INTO rating_anchor
                     (program_version_id, rating_scale_id, value, label, definition)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(version_id)
            .bind(scale_id)
            .bind(anchor.value)
            .bind(&anchor.label)
            .bind(&anchor.definition)
            .execute(&mut *conn)
            .await
            .context("inserting rating anchor")?;
        }
    }
    for modifier in &content.rating_modifiers {
        sqlx::query(
            "INSERT INTO rating_modifier (program_version_id, code, label, description)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(version_id)
        .bind(&modifier.code)
        .bind(&modifier.label)
        .bind(&modifier.description)
        .execute(&mut *conn)
        .await
        .context("inserting rating modifier")?;
    }
    Ok(scale_ids)
}

async fn insert_forms(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
    competency_ids: &HashMap<String, i64>,
    scale_ids: &HashMap<String, i64>,
) -> Result<()> {
    for form in &content.evaluation_forms {
        let result = sqlx::query(
            "INSERT INTO evaluation_form (program_version_id, record_type, name, instructions)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(version_id)
        .bind(form.record_type.as_str())
        .bind(&form.name)
        .bind(&form.instructions)
        .execute(&mut *conn)
        .await
        .context("inserting evaluation form")?;
        let form_id = result.last_insert_rowid();
        for (order, binding) in (0_i64..).zip(&form.competencies) {
            sqlx::query(
                "INSERT INTO form_competency
                     (program_version_id, evaluation_form_id, competency_id, rating_scale_id, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(version_id)
            .bind(form_id)
            .bind(competency_ids[&binding.competency])
            .bind(scale_ids[&binding.rating_scale])
            .bind(order)
            .execute(&mut *conn)
            .await
            .context("inserting form competency")?;
        }
        for (order, narrative) in (0_i64..).zip(&form.narratives) {
            sqlx::query(
                "INSERT INTO form_narrative
                     (program_version_id, evaluation_form_id, prompt, required, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(version_id)
            .bind(form_id)
            .bind(&narrative.prompt)
            .bind(i64::from(narrative.required))
            .bind(order)
            .execute(&mut *conn)
            .await
            .context("inserting form narrative")?;
        }
    }
    Ok(())
}

/// Writes every owned row of a validated content document.
pub(super) async fn insert_content(
    conn: &mut SqliteConnection,
    version_id: i64,
    content: &VersionContent,
) -> Result<()> {
    insert_phases(&mut *conn, version_id, content).await?;
    let competency_ids = insert_competencies(&mut *conn, version_id, content).await?;
    let scale_ids = insert_scales(&mut *conn, version_id, content).await?;
    insert_forms(&mut *conn, version_id, content, &competency_ids, &scale_ids).await?;
    for citation in &content.citations {
        insert_citation(&mut *conn, version_id, None, None, citation).await?;
    }
    sqlx::query(
        "INSERT INTO finalization_policy
             (program_version_id, review_approved, required_narratives, ratings_complete)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(version_id)
    .bind(i64::from(content.finalization_policy.review_approved))
    .bind(i64::from(content.finalization_policy.required_narratives))
    .bind(i64::from(content.finalization_policy.ratings_complete))
    .execute(&mut *conn)
    .await
    .context("writing finalization policy")?;
    Ok(())
}

/// Deletes every owned row of a draft version, children before parents so
/// foreign keys hold throughout.
pub(super) async fn delete_content(conn: &mut SqliteConnection, version_id: i64) -> Result<()> {
    for statement in [
        "DELETE FROM finalization_policy WHERE program_version_id = ?1",
        "DELETE FROM standards_citation WHERE program_version_id = ?1",
        "DELETE FROM form_narrative WHERE program_version_id = ?1",
        "DELETE FROM form_competency WHERE program_version_id = ?1",
        "DELETE FROM evaluation_form WHERE program_version_id = ?1",
        "DELETE FROM rating_anchor WHERE program_version_id = ?1",
        "DELETE FROM rating_scale WHERE program_version_id = ?1",
        "DELETE FROM rating_modifier WHERE program_version_id = ?1",
        "DELETE FROM task WHERE program_version_id = ?1",
        "DELETE FROM competency WHERE program_version_id = ?1",
        "DELETE FROM phase_transition WHERE program_version_id = ?1",
        "DELETE FROM phase WHERE program_version_id = ?1",
    ] {
        sqlx::query(statement)
            .bind(version_id)
            .execute(&mut *conn)
            .await
            .context("deleting draft content")?;
    }
    Ok(())
}

// ---- content reads

/// Loads a version's complete content document, or `None` when the
/// version does not exist. Arrays come back in the deterministic export
/// order (authored order where one exists, content order otherwise).
pub async fn load_content(pool: &SqlitePool, version_id: i64) -> Result<Option<VersionContent>> {
    // One transaction so every query reads the same snapshot.
    let mut tx = pool.begin().await.context("starting content load")?;
    let Some(header) =
        sqlx::query("SELECT name, label, description FROM program_version WHERE id = ?1")
            .bind(version_id)
            .fetch_optional(&mut *tx)
            .await
            .context("loading version row")?
    else {
        return Ok(None);
    };
    let mut content = VersionContent {
        name: header.get("name"),
        label: header.get("label"),
        description: header.get("description"),
        phases: Vec::new(),
        phase_transitions: Vec::new(),
        competencies: Vec::new(),
        rating_scales: Vec::new(),
        rating_modifiers: Vec::new(),
        evaluation_forms: Vec::new(),
        citations: Vec::new(),
        finalization_policy: PolicyDef::default(),
    };
    if let Some(policy) = sqlx::query(
        "SELECT review_approved, required_narratives, ratings_complete
         FROM finalization_policy WHERE program_version_id = ?1",
    )
    .bind(version_id)
    .fetch_optional(&mut *tx)
    .await
    .context("loading finalization policy")?
    {
        content.finalization_policy = PolicyDef {
            review_approved: policy.get::<i64, _>("review_approved") != 0,
            required_narratives: policy.get::<i64, _>("required_narratives") != 0,
            ratings_complete: policy.get::<i64, _>("ratings_complete") != 0,
        };
    }
    load_phases(&mut tx, version_id, &mut content).await?;
    let competency_index = load_competencies(&mut tx, version_id, &mut content).await?;
    load_scales(&mut tx, version_id, &mut content).await?;
    load_forms(&mut tx, version_id, &mut content).await?;
    load_citations(&mut tx, version_id, &mut content, &competency_index).await?;
    Ok(Some(content))
}

async fn load_phases(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT name, description, presentation_number FROM phase
         WHERE program_version_id = ?1 ORDER BY presentation_number, name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading phases")?;
    content.phases = rows
        .iter()
        .map(|row| PhaseDef {
            name: row.get("name"),
            description: row.get("description"),
            presentation_number: row.get("presentation_number"),
        })
        .collect();
    let rows = sqlx::query(
        "SELECT f.name AS from_name, t.name AS to_name, pt.kind
         FROM phase_transition pt
         JOIN phase f ON f.id = pt.from_phase_id
         JOIN phase t ON t.id = pt.to_phase_id
         WHERE pt.program_version_id = ?1
         ORDER BY f.name, t.name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading phase transitions")?;
    for row in &rows {
        content.phase_transitions.push(TransitionDef {
            from_phase: row.get("from_name"),
            to_phase: row.get("to_name"),
            kind: TransitionKind::from_db(row.get("kind"))?,
        });
    }
    Ok(())
}

/// Loads competencies and their tasks; returns row-id lookup maps used to
/// route citations.
async fn load_competencies(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
) -> Result<CompetencyIndex> {
    let rows = sqlx::query(
        "SELECT id, category, name, description FROM competency
         WHERE program_version_id = ?1 ORDER BY sort_order, name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading competencies")?;
    let mut index = CompetencyIndex::default();
    for row in &rows {
        let id: i64 = row.get("id");
        index
            .by_competency_row
            .insert(id, content.competencies.len());
        content.competencies.push(CompetencyDef {
            category: row.get("category"),
            name: row.get("name"),
            description: row.get("description"),
            tasks: Vec::new(),
            citations: Vec::new(),
        });
    }
    let rows = sqlx::query(
        "SELECT id, competency_id, prompt FROM task
         WHERE program_version_id = ?1 ORDER BY sort_order, prompt",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading tasks")?;
    for row in &rows {
        let competency_row: i64 = row.get("competency_id");
        let competency_slot = index.by_competency_row[&competency_row];
        let tasks = &mut content.competencies[competency_slot].tasks;
        index
            .by_task_row
            .insert(row.get("id"), (competency_slot, tasks.len()));
        tasks.push(TaskDef {
            prompt: row.get("prompt"),
            citations: Vec::new(),
        });
    }
    Ok(index)
}

#[derive(Default)]
struct CompetencyIndex {
    /// competency row id -> index into `content.competencies`
    by_competency_row: HashMap<i64, usize>,
    /// task row id -> (competency index, task index)
    by_task_row: HashMap<i64, (usize, usize)>,
}

async fn load_scales(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, name, kind, min_value, max_value FROM rating_scale
         WHERE program_version_id = ?1 ORDER BY name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading rating scales")?;
    let mut slot_by_row: HashMap<i64, usize> = HashMap::new();
    for row in &rows {
        slot_by_row.insert(row.get("id"), content.rating_scales.len());
        content.rating_scales.push(ScaleDef {
            name: row.get("name"),
            kind: ScaleKind::from_db(row.get("kind"))?,
            min_value: row.get("min_value"),
            max_value: row.get("max_value"),
            anchors: Vec::new(),
        });
    }
    let rows = sqlx::query(
        "SELECT rating_scale_id, value, label, definition FROM rating_anchor
         WHERE program_version_id = ?1 ORDER BY value",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading rating anchors")?;
    for row in &rows {
        let scale_row: i64 = row.get("rating_scale_id");
        content.rating_scales[slot_by_row[&scale_row]]
            .anchors
            .push(AnchorDef {
                value: row.get("value"),
                label: row.get("label"),
                definition: row.get("definition"),
            });
    }
    let rows = sqlx::query(
        "SELECT code, label, description FROM rating_modifier
         WHERE program_version_id = ?1 ORDER BY code",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading rating modifiers")?;
    content.rating_modifiers = rows
        .iter()
        .map(|row| ModifierDef {
            code: row.get("code"),
            label: row.get("label"),
            description: row.get("description"),
        })
        .collect();
    Ok(())
}

async fn load_forms(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, record_type, name, instructions FROM evaluation_form
         WHERE program_version_id = ?1 ORDER BY name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading evaluation forms")?;
    let mut slot_by_row: HashMap<i64, usize> = HashMap::new();
    for row in &rows {
        slot_by_row.insert(row.get("id"), content.evaluation_forms.len());
        content.evaluation_forms.push(FormDef {
            record_type: RecordType::from_db(row.get("record_type"))?,
            name: row.get("name"),
            instructions: row.get("instructions"),
            competencies: Vec::new(),
            narratives: Vec::new(),
        });
    }
    let rows = sqlx::query(
        "SELECT fc.evaluation_form_id, c.name AS competency, s.name AS rating_scale
         FROM form_competency fc
         JOIN competency c ON c.id = fc.competency_id
         JOIN rating_scale s ON s.id = fc.rating_scale_id
         WHERE fc.program_version_id = ?1
         ORDER BY fc.sort_order, c.name",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading form competencies")?;
    for row in &rows {
        let form_row: i64 = row.get("evaluation_form_id");
        content.evaluation_forms[slot_by_row[&form_row]]
            .competencies
            .push(FormCompetencyDef {
                competency: row.get("competency"),
                rating_scale: row.get("rating_scale"),
            });
    }
    let rows = sqlx::query(
        "SELECT evaluation_form_id, prompt, required FROM form_narrative
         WHERE program_version_id = ?1 ORDER BY sort_order, prompt",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading form narratives")?;
    for row in &rows {
        let form_row: i64 = row.get("evaluation_form_id");
        let required: i64 = row.get("required");
        content.evaluation_forms[slot_by_row[&form_row]]
            .narratives
            .push(NarrativeDef {
                prompt: row.get("prompt"),
                required: required != 0,
            });
    }
    Ok(())
}

async fn load_citations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    version_id: i64,
    content: &mut VersionContent,
    index: &CompetencyIndex,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT competency_id, task_id, body, edition, clause, note
         FROM standards_citation WHERE program_version_id = ?1
         ORDER BY body, edition, clause, note",
    )
    .bind(version_id)
    .fetch_all(&mut **tx)
    .await
    .context("loading standards citations")?;
    for row in &rows {
        let citation = CitationDef {
            body: row.get("body"),
            edition: row.get("edition"),
            clause: row.get("clause"),
            note: row.get("note"),
        };
        let competency_id: Option<i64> = row.get("competency_id");
        let task_id: Option<i64> = row.get("task_id");
        match (competency_id, task_id) {
            (Some(competency_row), None) => {
                let slot = index.by_competency_row[&competency_row];
                content.competencies[slot].citations.push(citation);
            }
            (None, Some(task_row)) => {
                let (competency_slot, task_slot) = index.by_task_row[&task_row];
                content.competencies[competency_slot].tasks[task_slot]
                    .citations
                    .push(citation);
            }
            (None, None) => content.citations.push(citation),
            (Some(_), Some(_)) => {
                bail!("citation targets both a competency and a task; the schema forbids this")
            }
        }
    }
    Ok(())
}
