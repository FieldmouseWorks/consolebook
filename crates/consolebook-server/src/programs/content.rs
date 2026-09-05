//! Program configuration vocabulary and structural validation.

use std::collections::HashSet;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

// ---- content document
//
// The same typed document is the authoring input (`replace_draft`), the
// read model (`load_content`), and the export/import payload
// (`program_export`). Strings are stored verbatim; required fields must
// contain non-whitespace content. References between parts use exact
// names, and name uniqueness is ASCII-case-insensitive to match the
// database's NOCASE indexes.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionContent {
    /// Snapshot of the program name as presented by this version. A later
    /// program rename never rewrites it.
    pub name: String,
    /// Agency-visible free-text label; presentation, never identity.
    pub label: String,
    pub description: String,
    pub phases: Vec<PhaseDef>,
    pub phase_transitions: Vec<TransitionDef>,
    pub competencies: Vec<CompetencyDef>,
    pub rating_scales: Vec<ScaleDef>,
    pub rating_modifiers: Vec<ModifierDef>,
    pub evaluation_forms: Vec<FormDef>,
    /// Version-level standards citations; competency- and task-level
    /// citations nest under their owners.
    pub citations: Vec<CitationDef>,
    /// Completion rules gating finalization (ADR 0011), versioned like
    /// every other piece of configuration. Absent in older exports;
    /// the conservative defaults apply.
    #[serde(default)]
    pub finalization_policy: PolicyDef,
}

/// The closed v1 completion-rule set (#32 decision 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDef {
    pub review_approved: bool,
    pub required_narratives: bool,
    pub ratings_complete: bool,
}

impl Default for PolicyDef {
    fn default() -> Self {
        Self {
            review_approved: true,
            required_narratives: true,
            ratings_complete: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseDef {
    pub name: String,
    pub description: String,
    /// Presentation data (docs/domain-model.md): ordering, never progress.
    pub presentation_number: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionDef {
    pub from_phase: String,
    pub to_phase: String,
    pub kind: TransitionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Advance,
    Remediation,
    Skip,
    Restart,
}

impl TransitionKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Advance => "advance",
            Self::Remediation => "remediation",
            Self::Skip => "skip",
            Self::Restart => "restart",
        }
    }

    pub(super) fn from_db(value: &str) -> Result<Self> {
        match value {
            "advance" => Ok(Self::Advance),
            "remediation" => Ok(Self::Remediation),
            "skip" => Ok(Self::Skip),
            "restart" => Ok(Self::Restart),
            other => bail!("unknown transition kind in database: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompetencyDef {
    /// Free-text grouping label; empty means uncategorized.
    pub category: String,
    pub name: String,
    pub description: String,
    pub tasks: Vec<TaskDef>,
    pub citations: Vec<CitationDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDef {
    pub prompt: String,
    pub citations: Vec<CitationDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScaleDef {
    pub name: String,
    pub kind: ScaleKind,
    /// Present exactly when `kind` is `anchored_numeric`.
    pub min_value: Option<i64>,
    /// Present exactly when `kind` is `anchored_numeric`.
    pub max_value: Option<i64>,
    pub anchors: Vec<AnchorDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleKind {
    AnchoredNumeric,
    PassFail,
    NarrativeOnly,
}

impl ScaleKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnchoredNumeric => "anchored_numeric",
            Self::PassFail => "pass_fail",
            Self::NarrativeOnly => "narrative_only",
        }
    }

    pub(super) fn from_db(value: &str) -> Result<Self> {
        match value {
            "anchored_numeric" => Ok(Self::AnchoredNumeric),
            "pass_fail" => Ok(Self::PassFail),
            "narrative_only" => Ok(Self::NarrativeOnly),
            other => bail!("unknown rating scale kind in database: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorDef {
    pub value: i64,
    pub label: String,
    pub definition: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModifierDef {
    pub code: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormDef {
    pub record_type: RecordType,
    pub name: String,
    pub instructions: String,
    pub competencies: Vec<FormCompetencyDef>,
    pub narratives: Vec<NarrativeDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordType {
    DailyReport,
    WeeklySummary,
    PhaseEvaluation,
}

impl RecordType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DailyReport => "daily_report",
            Self::WeeklySummary => "weekly_summary",
            Self::PhaseEvaluation => "phase_evaluation",
        }
    }

    pub(super) fn from_db(value: &str) -> Result<Self> {
        match value {
            "daily_report" => Ok(Self::DailyReport),
            "weekly_summary" => Ok(Self::WeeklySummary),
            "phase_evaluation" => Ok(Self::PhaseEvaluation),
            other => bail!("unknown record type in database: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormCompetencyDef {
    /// Exact name of a competency defined in this version.
    pub competency: String,
    /// Exact name of a rating scale defined in this version.
    pub rating_scale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NarrativeDef {
    pub prompt: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CitationDef {
    /// Standards body, e.g. an accreditation program name.
    pub body: String,
    /// Edition or revision of the cited standard; may be empty.
    pub edition: String,
    pub clause: String,
    pub note: String,
}

// ---- validation

fn blank(value: &str) -> bool {
    value.trim().is_empty()
}

/// Detects a duplicate under the database's ASCII-case-insensitive
/// uniqueness rules.
fn note_duplicate(seen: &mut HashSet<String>, value: &str) -> bool {
    !seen.insert(value.to_ascii_lowercase())
}

fn validate_citations(problems: &mut Vec<String>, owner: &str, citations: &[CitationDef]) {
    for citation in citations {
        if blank(&citation.body) {
            problems.push(format!("{owner}: citation has an empty standards body"));
        }
        if blank(&citation.clause) {
            problems.push(format!("{owner}: citation has an empty clause"));
        }
    }
}

fn validate_phases(problems: &mut Vec<String>, content: &VersionContent) {
    let mut names = HashSet::new();
    for phase in &content.phases {
        if blank(&phase.name) {
            problems.push("phase has an empty name".to_owned());
        } else if note_duplicate(&mut names, &phase.name) {
            problems.push(format!("duplicate phase name '{}'", phase.name));
        }
    }
    let defined: HashSet<&str> = content.phases.iter().map(|p| p.name.as_str()).collect();
    let mut edges = HashSet::new();
    for transition in &content.phase_transitions {
        for endpoint in [&transition.from_phase, &transition.to_phase] {
            if !defined.contains(endpoint.as_str()) {
                problems.push(format!("transition references unknown phase '{endpoint}'"));
            }
        }
        if !edges.insert((transition.from_phase.clone(), transition.to_phase.clone())) {
            problems.push(format!(
                "duplicate transition from '{}' to '{}'",
                transition.from_phase, transition.to_phase
            ));
        }
    }
}

fn validate_competencies(problems: &mut Vec<String>, content: &VersionContent) {
    let mut names = HashSet::new();
    for competency in &content.competencies {
        if blank(&competency.name) {
            problems.push("competency has an empty name".to_owned());
            continue;
        }
        if note_duplicate(&mut names, &competency.name) {
            problems.push(format!("duplicate competency name '{}'", competency.name));
        }
        let mut prompts = HashSet::new();
        for task in &competency.tasks {
            if blank(&task.prompt) {
                problems.push(format!(
                    "competency '{}': task has an empty prompt",
                    competency.name
                ));
            } else if note_duplicate(&mut prompts, &task.prompt) {
                problems.push(format!(
                    "competency '{}': duplicate task prompt '{}'",
                    competency.name, task.prompt
                ));
            }
            validate_citations(
                problems,
                &format!("task '{}'", task.prompt),
                &task.citations,
            );
        }
        validate_citations(
            problems,
            &format!("competency '{}'", competency.name),
            &competency.citations,
        );
    }
}

fn validate_scale(problems: &mut Vec<String>, scale: &ScaleDef) {
    let mut values = HashSet::new();
    for anchor in &scale.anchors {
        if blank(&anchor.label) {
            problems.push(format!("scale '{}': anchor has an empty label", scale.name));
        }
        if !values.insert(anchor.value) {
            problems.push(format!(
                "scale '{}': duplicate anchor value {}",
                scale.name, anchor.value
            ));
        }
    }
    match scale.kind {
        ScaleKind::AnchoredNumeric => {
            let (Some(min), Some(max)) = (scale.min_value, scale.max_value) else {
                problems.push(format!(
                    "scale '{}': anchored_numeric requires min_value and max_value",
                    scale.name
                ));
                return;
            };
            if min >= max {
                problems.push(format!(
                    "scale '{}': min_value must be less than max_value",
                    scale.name
                ));
            }
            if scale.anchors.is_empty() {
                problems.push(format!(
                    "scale '{}': anchored_numeric requires at least one anchor",
                    scale.name
                ));
            }
            for anchor in &scale.anchors {
                if anchor.value < min || anchor.value > max {
                    problems.push(format!(
                        "scale '{}': anchor value {} is outside {min}..={max}",
                        scale.name, anchor.value
                    ));
                }
            }
        }
        ScaleKind::PassFail => {
            if scale.min_value.is_some() || scale.max_value.is_some() {
                problems.push(format!(
                    "scale '{}': pass_fail does not take numeric bounds",
                    scale.name
                ));
            }
            let values: Vec<i64> = scale.anchors.iter().map(|a| a.value).collect();
            if !(values.len() == 2 && values.contains(&0) && values.contains(&1)) {
                problems.push(format!(
                    "scale '{}': pass_fail requires exactly two anchors with values 0 and 1",
                    scale.name
                ));
            }
        }
        ScaleKind::NarrativeOnly => {
            if scale.min_value.is_some() || scale.max_value.is_some() {
                problems.push(format!(
                    "scale '{}': narrative_only does not take numeric bounds",
                    scale.name
                ));
            }
            if !scale.anchors.is_empty() {
                problems.push(format!(
                    "scale '{}': narrative_only takes no anchors",
                    scale.name
                ));
            }
        }
    }
}

fn validate_forms(problems: &mut Vec<String>, content: &VersionContent) {
    let competencies: HashSet<&str> = content
        .competencies
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let scales: HashSet<&str> = content
        .rating_scales
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let mut names = HashSet::new();
    for form in &content.evaluation_forms {
        if blank(&form.name) {
            problems.push("evaluation form has an empty name".to_owned());
            continue;
        }
        if note_duplicate(&mut names, &form.name) {
            problems.push(format!("duplicate evaluation form name '{}'", form.name));
        }
        let mut bound = HashSet::new();
        for binding in &form.competencies {
            if !competencies.contains(binding.competency.as_str()) {
                problems.push(format!(
                    "form '{}': references unknown competency '{}'",
                    form.name, binding.competency
                ));
            }
            if !scales.contains(binding.rating_scale.as_str()) {
                problems.push(format!(
                    "form '{}': references unknown rating scale '{}'",
                    form.name, binding.rating_scale
                ));
            }
            if !bound.insert(binding.competency.clone()) {
                problems.push(format!(
                    "form '{}': competency '{}' is bound more than once",
                    form.name, binding.competency
                ));
            }
        }
        for narrative in &form.narratives {
            if blank(&narrative.prompt) {
                problems.push(format!(
                    "form '{}': narrative has an empty prompt",
                    form.name
                ));
            }
        }
    }
}

/// Structural validation of a content document: required text present,
/// names unique under the database's case-insensitive rules, and every
/// cross-reference resolving inside the document. Returns problems;
/// empty means valid.
#[must_use]
pub fn validate_content(content: &VersionContent) -> Vec<String> {
    let mut problems = Vec::new();
    if blank(&content.name) {
        problems.push("version has an empty program name".to_owned());
    }
    validate_phases(&mut problems, content);
    validate_competencies(&mut problems, content);
    let mut scale_names = HashSet::new();
    for scale in &content.rating_scales {
        if blank(&scale.name) {
            problems.push("rating scale has an empty name".to_owned());
            continue;
        }
        if note_duplicate(&mut scale_names, &scale.name) {
            problems.push(format!("duplicate rating scale name '{}'", scale.name));
        }
        validate_scale(&mut problems, scale);
    }
    let mut codes = HashSet::new();
    for modifier in &content.rating_modifiers {
        if blank(&modifier.code) || blank(&modifier.label) {
            problems.push("rating modifier has an empty code or label".to_owned());
        } else if note_duplicate(&mut codes, &modifier.code) {
            problems.push(format!(
                "duplicate rating modifier code '{}'",
                modifier.code
            ));
        }
    }
    validate_forms(&mut problems, content);
    validate_citations(&mut problems, "version", &content.citations);
    problems
}
