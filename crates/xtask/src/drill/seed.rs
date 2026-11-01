//! `cargo xtask drill seed`: a fresh installation set up over its HTTP API
//! with invented people and records. Three daily evaluations are finalized,
//! and the trainee acknowledges two of them, one with a response.

use std::path::Path;

use serde_json::{Value, json};
use ureq::http::Method;

use super::account::Account;
use super::client::Client;
use super::{DrillError, field, shown};

const AGENCY: &str = "Example County Communications";
const DATES: [&str; 3] = ["2026-06-02", "2026-06-03", "2026-06-04"];

/// Someone at the invented agency.
struct Person {
    username: &'static str,
    display_name: &'static str,
    /// The role an administrator creates them with; `None` is the default.
    role: Option<&'static str>,
}

const ADMIN: Person = Person {
    username: "avery.admin",
    display_name: "Avery Admin",
    role: None,
};
const TRAINEE: Person = Person {
    username: "taylor.trainee",
    display_name: "Taylor Trainee",
    role: None,
};
const TRAINER: Person = Person {
    username: "jordan.trainer",
    display_name: "Jordan Trainer",
    role: Some("trainer"),
};
const COORDINATOR: Person = Person {
    username: "casey.coord",
    display_name: "Casey Coordinator",
    role: Some("coordinator"),
};

/// The invented program: one phase, one competency, a 1-7 scale and a daily
/// report form.
fn program() -> Value {
    json!({
        "name": "Example County CTO Program",
        "label": "2026 rev A",
        "description": "Invented program for a restore drill.",
        "phases": [{ "name": "Phase One", "description": "Observation.", "presentation_number": 1 }],
        "phase_transitions": [],
        "competencies": [{
            "category": "Call processing",
            "name": "Emergency Call Interrogation",
            "description": "Obtains and verifies location, callback, and nature.",
            "tasks": [{ "prompt": "Processes an invented structure-fire call.", "citations": [] }],
            "citations": [],
        }],
        "rating_scales": [{
            "name": "Standard 1-7",
            "kind": "anchored_numeric",
            "min_value": 1,
            "max_value": 7,
            "anchors": [
                { "value": 1, "label": "Unacceptable", "definition": "Contrary to training." },
                { "value": 4, "label": "Meets standards", "definition": "To the invented standard." },
            ],
        }],
        "rating_modifiers": [],
        "evaluation_forms": [{
            "record_type": "daily_report",
            "name": "Daily Observation Report",
            "instructions": "Rate observed performance.",
            "competencies": [{ "competency": "Emergency Call Interrogation", "rating_scale": "Standard 1-7" }],
            "narratives": [
                { "prompt": "Most acceptable performance.", "required": false },
                { "prompt": "Least acceptable performance.", "required": false },
            ],
        }],
        "citations": [],
        "finalization_policy": { "review_approved": false, "required_narratives": false, "ratings_complete": false },
    })
}

/// What [`seed`] set up.
#[derive(Debug)]
pub struct Seeded {
    /// The administrator, as `account` holds it.
    pub admin: Account,
    pub enrollment: i64,
    pub records: usize,
}

/// Sets up the fresh installation at `base` with `setup_code`, and writes
/// its administrator's sign-in to `account`, a new file, before anything
/// else: the installation is never left with an administrator nobody can
/// sign in as.
pub fn seed(base: &str, setup_code: &str, account: &Path) -> Result<Seeded, DrillError> {
    let admin_account = Account::invented(ADMIN.username)?;
    admin_account.write(account)?;
    Client::new(base).call(
        &Method::POST,
        "/api/setup",
        Some(&json!({
            "setup_code": setup_code,
            "agency_name": AGENCY,
            "username": ADMIN.username,
            "display_name": ADMIN.display_name,
            "password": admin_account.passphrase,
        })),
    )?;
    let mut admin = Client::sign_in(base, &admin_account)?;
    let program_id = id(&admin.call(
        &Method::POST,
        "/api/programs",
        Some(&json!({ "name": program()["name"] })),
    )?)?;
    let version = id(&admin.call(
        &Method::POST,
        &format!("/api/programs/{program_id}/versions"),
        Some(&program()),
    )?)?;
    admin.call(
        &Method::POST,
        &format!("/api/program-versions/{version}/publish"),
        Some(&json!({})),
    )?;
    let (trainee_id, mut trainee) = user(&mut admin, base, &TRAINEE)?;
    // The trainer evaluates; `trainee` and `trainer` are too alike to read.
    let (evaluator_id, mut evaluator) = user(&mut admin, base, &TRAINER)?;
    let (_, mut coordinator) = user(&mut admin, base, &COORDINATOR)?;
    let enrollment = id(&admin.call(
        &Method::POST,
        &format!("/api/program-versions/{version}/enrollments"),
        Some(&json!({ "user_id": trainee_id })),
    )?)?;
    admin.call(
        &Method::POST,
        &format!("/api/enrollments/{enrollment}/assignments"),
        Some(&json!({ "trainer_user_id": evaluator_id })),
    )?;
    for (n, date) in DATES.iter().enumerate() {
        let session = id(&admin.call(
            &Method::POST,
            &format!("/api/enrollments/{enrollment}/sessions"),
            Some(&json!({
                "business_date": date,
                "timezone": "America/Chicago",
                "local_start": format!("{date}T07:00"),
                "trainer_user_ids": [evaluator_id],
            })),
        )?)?;
        evaluator.call(
            &Method::POST,
            &format!("/api/sessions/{session}/close"),
            Some(&json!({ "disposition": "completed", "local_end": format!("{date}T17:00") })),
        )?;
        let draft = id(&admin.call(
            &Method::POST,
            &format!("/api/sessions/{session}/draft"),
            Some(&json!({})),
        )?)?;
        let view = evaluator.call(&Method::GET, &format!("/api/drafts/{draft}"), None)?;
        let saved = evaluator.call(
            &Method::PUT,
            &format!("/api/drafts/{draft}/content"),
            Some(&content(n, &view)?),
        )?;
        coordinator.call(
            &Method::POST,
            &format!("/api/drafts/{draft}/finalize"),
            Some(&json!({ "revision": field(&saved, "revision")? })),
        )?;
        let acknowledgment = match n {
            0 => Some(json!({ "kind": "acknowledged" })),
            1 => Some(json!({
                "kind": "acknowledged_with_response",
                "response": "Invented trainee response.",
            })),
            _ => None,
        };
        if let Some(acknowledgment) = acknowledgment {
            trainee.call(
                &Method::POST,
                &format!("/api/drafts/{draft}/acknowledge"),
                Some(&acknowledgment),
            )?;
        }
    }
    Ok(Seeded {
        admin: admin_account,
        enrollment,
        records: DATES.len(),
    })
}

/// The `n`th record's ratings and narratives, on the draft `view` shows.
fn content(n: usize, view: &Value) -> Result<Value, DrillError> {
    let form = field(view, "form")?;
    let ratings: Vec<Value> = list(form, "competencies")?
        .iter()
        .map(|c| {
            Ok(json!({
                "form_competency_id": field(c, "form_competency_id")?,
                "value": 4 + n % 2,
            }))
        })
        .collect::<Result<_, DrillError>>()?;
    let narratives: Vec<Value> = list(form, "narratives")?
        .iter()
        .map(|r| {
            Ok(json!({
                "form_narrative_id": field(r, "form_narrative_id")?,
                "text": format!(
                    "Invented observation {}: {}",
                    n + 1,
                    shown(field(r, "prompt")?).to_lowercase()
                ),
            }))
        })
        .collect::<Result<_, DrillError>>()?;
    Ok(json!({
        "revision": field(view, "revision")?,
        "ratings": ratings,
        "narratives": narratives,
    }))
}

/// Creates `who` as `admin`, sets a password made for them through the
/// reset code, and signs them in: their id and their client.
fn user(admin: &mut Client, base: &str, who: &Person) -> Result<(i64, Client), DrillError> {
    let mut body = json!({ "username": who.username, "display_name": who.display_name });
    if let Some(role) = who.role {
        body["role"] = role.into();
    }
    let created = admin.call(&Method::POST, "/api/users", Some(&body))?;
    let account = Account::invented(who.username)?;
    Client::new(base).call(
        &Method::POST,
        "/api/auth/reset",
        Some(&json!({
            "username": who.username,
            "reset_code": field(&created, "reset_code")?,
            "new_password": account.passphrase,
        })),
    )?;
    Ok((id(&created)?, Client::sign_in(base, &account)?))
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, DrillError> {
    field(value, key)?
        .as_array()
        .ok_or_else(|| DrillError(format!("expected a list in `{key}` of {value}")))
}

fn id(value: &Value) -> Result<i64, DrillError> {
    field(value, "id")?
        .as_i64()
        .ok_or_else(|| DrillError(format!("expected a whole-number `id` in {value}")))
}
