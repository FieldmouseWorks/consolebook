# Installation Export Format

Everything an installation holds, except credentials, as one verifiable
archive (ADR 0028; #98; #44 decision 1; `docs/records-integrity.md`).
`consolebook-server/src/installation_export/` produces it,
`consolebook-server/src/installation_verify.rs` verifies it, and
`tests/installation_export.rs` proves its contents, counts, determinism, and
verification findings. This document is normative: the implementation follows
it, not the other way around.

The archive has two parts:

- **units:** every finalized version as a record-export unit, byte for byte;
- **table documents:** one canonical-JSON document for every other stored
  table, with each row's ids kept as references.

The record export (`docs/formats/record-export.md`) carries finalized records
only. The trainee packet (`docs/formats/trainee-packet.md`) is one trainee's
history in a readable shape. This export is the installation's whole content
in the shape it is stored. An agency can leave with it, move it to another
system, or answer a records request from it after the installation is gone.

## Container

The container rules of the record export apply unchanged. The archive is a ZIP
with stored entries and files only, ASCII names, and the export instant as
every entry's modification time. Every entry has mode `0644`, and the archive
holds nothing the manifest does not name. Entry order is the manifest, then
units in ascending (`record_id`, `version_number`) order, then table documents
in ascending name order.

```text
manifest.json                              installation manifest
records/{record_id}/v{n}/record.json       canonical record bytes (as the record export)
records/{record_id}/v{n}/manifest.json     unit manifest (as the record export)
tables/{table}.json                        one document per carried table
```

## Manifest (`manifest.json`)

```json
{
  "excluded": [
    { "name": "password_reset_code", "reason": "credential" },
    { "name": "session", "reason": "credential" },
    { "name": "session_secure_cookie", "reason": "credential" },
    { "name": "setup_code", "reason": "credential" },
    { "name": "user.password_hash", "reason": "credential" }
  ],
  "exported_at": 1788289200,
  "format": "consolebook-installation-export",
  "format_version": 1,
  "installation_id": "…",
  "tables": [
    { "name": "acknowledgment", "path": "tables/acknowledgment.json", "rows": 12, "sha256": "…" },
    { "name": "agency", "path": "tables/agency.json", "rows": 1, "sha256": "…" }
  ],
  "units": [ … ]
}
```

| Member | Type | Meaning |
| --- | --- | --- |
| `format` | string | Always `consolebook-installation-export` |
| `format_version` | integer | `1`; bumped by any change to the manifest, the table set, or any document's columns, types, or rules |
| `installation_id` | string | The exporting installation's identity |
| `exported_at` | integer | The export instant, UTC Unix seconds. It is identical in every unit manifest |
| `units` | array | Every finalized version the installation holds, listed exactly as the record export lists units. It may be empty |
| `tables` | array | Every carried table, ascending by `name`, each with its document's `path` (`tables/{name}.json`), its row count, and the SHA-256 of its bytes (64 lowercase hex characters) |
| `excluded` | array | Every table or column the format does not carry, ascending by `name`, each with its `reason` |

The `tables` and `excluded` names together are the **ledger**. For a given
format version the ledger is a closed set. Version 1 has 43 carried tables and
5 exclusions (four tables and one column), listed below. A name outside the
set, a name missing from it, or a name listed twice is a finding. The ledger
makes "nothing silently omitted" a property the archive states and the
verifier checks.

`reason` is one of:

- `credential`: the value would let someone sign in, or it is the hash of a
  secret that would. It means nothing outside this installation, and anyone
  holding a copy of the archive could misuse it.

The manifest is canonical JSON under the record format's JCS subset (ADR 0011),
as is every document in the archive.

## Units

Units are record-export units. Everything `docs/formats/record-export.md` says
about `record.json`, unit manifests, and unit verification holds here without
restatement. The installation's units are every finalized version it holds,
superseded originals included. Every unit's predecessor is therefore carried:
a version whose predecessor is absent is a hole in the lineage
(*predecessor not carried*), as it is in a trainee packet.

The `evaluation_version` table document lists the same versions as the units,
row for row. It carries every stored column except `canonical_bytes`, which
travel as the unit's `record.json`. The units and that document must agree
both ways:

- every row has exactly one unit, at
  `records/{evaluation_record_id}/v{version_number}`, and every unit has
  exactly one row;
- the unit's `record_schema`, `content_hash`, and `chain_hash` equal the row's;
- the unit's `predecessor_content_hash` is the `content_hash` of the row
  `predecessor_id` names, and `null` exactly when `predecessor_id` is `null`.

## Table documents

`tables/{table}.json` is a canonical-JSON array holding one object per stored
row. The rules:

- **Members.** A row object has exactly the columns listed for its table
  below, by their stored names. No column is missing, and there are no extra
  members.
- **Types.** An `integer` column is a JSON integer (the canonical subset
  carries magnitudes below 2^53; a stored value outside it is refused at
  production). A `text` column is a JSON
  string, carried exactly as stored, never trimmed or re-encoded. A `flag`
  column is `true` or `false` (stored as `1` or `0`). A nullable column is
  `null` when nothing is stored; it is never omitted.
- **Order.** Rows are strictly ascending by the table's key. Most keys are
  `id`. `capability_grant` is ordered by (`user_id`, `capability`) with
  strings in code-point order, `finalization_policy` by `program_version_id`,
  and `hold_release` by `hold_id`. A reordered or duplicated row is a finding.
- **Counts.** The array's length equals the manifest's `rows` for that table.

`draft_snapshot.content` is the draft content the installation stored when the
snapshot was taken, as a string. The export does not parse or re-serialize it.

`amendment.opened_after_event_id` and `amendment.opened_after_decision_id` are
workflow watermarks, not references: the highest `contributor_event` and
`review_decision` ids of the record when the amendment opened, or `0` when
there were none (migration 0012). The verifier does not resolve them.

### Columns

Each table's columns are listed in stored order, with these meanings:

- **Type** is `integer`, `text`, or `flag`.
- **Null** says whether the column may be `null`.
- **Rule**, where present, names the key, the table a reference resolves into
  (→), the closed vocabulary a value is drawn from, or a fixed text form.

#### Installation

| Table | Column | Type | Null | Rule |
| --- | --- | --- | --- | --- |
| `instance` | `id` | integer | no | key |
| `instance` | `installation_id` | text | no |  |
| `instance` | `created_at_utc` | text | no |  |
| `agency` | `id` | integer | no | key |
| `agency` | `name` | text | no |  |
| `agency` | `created_at` | integer | no |  |

#### People and access

| Table | Column | Type | Null | Rule |
| --- | --- | --- | --- | --- |
| `user` | `id` | integer | no | key |
| `user` | `username` | text | no |  |
| `user` | `display_name` | text | no |  |
| `user` | `created_at` | integer | no |  |
| `user` | `employee_id` | text | no |  |
| `user` | `title` | text | no |  |
| `capability_grant` | `user_id` | integer | no | key (1); → `user` |
| `capability_grant` | `capability` | text | no | key (2); capability (see Vocabularies) |
| `capability_grant` | `granted_at` | integer | no |  |
| `capability_grant` | `granted_by` | integer | yes | → `user` |
| `retention_authority_event` | `id` | integer | no | key |
| `retention_authority_event` | `user_id` | integer | no | → `user` |
| `retention_authority_event` | `granted` | flag | no |  |
| `retention_authority_event` | `actor_user_id` | integer | no | → `user` |
| `retention_authority_event` | `reason` | text | no |  |
| `retention_authority_event` | `recorded_at` | integer | no |  |

#### Program configuration

| Table | Column | Type | Null | Rule |
| --- | --- | --- | --- | --- |
| `program` | `id` | integer | no | key |
| `program` | `name` | text | no |  |
| `program` | `created_at` | integer | no |  |
| `program` | `created_by` | integer | yes | → `user` |
| `program_version` | `id` | integer | no | key |
| `program_version` | `program_id` | integer | no | → `program` |
| `program_version` | `version_number` | integer | no |  |
| `program_version` | `label` | text | no |  |
| `program_version` | `name` | text | no |  |
| `program_version` | `description` | text | no |  |
| `program_version` | `created_at` | integer | no |  |
| `program_version` | `created_by` | integer | yes | → `user` |
| `program_version` | `published_at` | integer | yes |  |
| `program_version` | `published_by` | integer | yes | → `user` |
| `phase` | `id` | integer | no | key |
| `phase` | `program_version_id` | integer | no | → `program_version` |
| `phase` | `name` | text | no |  |
| `phase` | `description` | text | no |  |
| `phase` | `presentation_number` | integer | no |  |
| `phase_transition` | `id` | integer | no | key |
| `phase_transition` | `program_version_id` | integer | no | → `program_version` |
| `phase_transition` | `from_phase_id` | integer | no | → `phase` |
| `phase_transition` | `to_phase_id` | integer | no | → `phase` |
| `phase_transition` | `kind` | text | no | one of `advance`, `remediation`, `skip`, `restart` |
| `competency` | `id` | integer | no | key |
| `competency` | `program_version_id` | integer | no | → `program_version` |
| `competency` | `category` | text | no |  |
| `competency` | `name` | text | no |  |
| `competency` | `description` | text | no |  |
| `competency` | `sort_order` | integer | no |  |
| `task` | `id` | integer | no | key |
| `task` | `program_version_id` | integer | no | → `program_version` |
| `task` | `competency_id` | integer | no | → `competency` |
| `task` | `prompt` | text | no |  |
| `task` | `sort_order` | integer | no |  |
| `standards_citation` | `id` | integer | no | key |
| `standards_citation` | `program_version_id` | integer | no | → `program_version` |
| `standards_citation` | `competency_id` | integer | yes | → `competency` |
| `standards_citation` | `task_id` | integer | yes | → `task` |
| `standards_citation` | `body` | text | no |  |
| `standards_citation` | `edition` | text | no |  |
| `standards_citation` | `clause` | text | no |  |
| `standards_citation` | `note` | text | no |  |
| `rating_scale` | `id` | integer | no | key |
| `rating_scale` | `program_version_id` | integer | no | → `program_version` |
| `rating_scale` | `name` | text | no |  |
| `rating_scale` | `kind` | text | no | one of `anchored_numeric`, `pass_fail`, `narrative_only` |
| `rating_scale` | `min_value` | integer | yes |  |
| `rating_scale` | `max_value` | integer | yes |  |
| `rating_anchor` | `id` | integer | no | key |
| `rating_anchor` | `program_version_id` | integer | no | → `program_version` |
| `rating_anchor` | `rating_scale_id` | integer | no | → `rating_scale` |
| `rating_anchor` | `value` | integer | no |  |
| `rating_anchor` | `label` | text | no |  |
| `rating_anchor` | `definition` | text | no |  |
| `rating_modifier` | `id` | integer | no | key |
| `rating_modifier` | `program_version_id` | integer | no | → `program_version` |
| `rating_modifier` | `code` | text | no |  |
| `rating_modifier` | `label` | text | no |  |
| `rating_modifier` | `description` | text | no |  |
| `evaluation_form` | `id` | integer | no | key |
| `evaluation_form` | `program_version_id` | integer | no | → `program_version` |
| `evaluation_form` | `record_type` | text | no | one of `daily_report`, `weekly_summary`, `phase_evaluation` |
| `evaluation_form` | `name` | text | no |  |
| `evaluation_form` | `instructions` | text | no |  |
| `form_competency` | `id` | integer | no | key |
| `form_competency` | `program_version_id` | integer | no | → `program_version` |
| `form_competency` | `evaluation_form_id` | integer | no | → `evaluation_form` |
| `form_competency` | `competency_id` | integer | no | → `competency` |
| `form_competency` | `rating_scale_id` | integer | no | → `rating_scale` |
| `form_competency` | `sort_order` | integer | no |  |
| `form_narrative` | `id` | integer | no | key |
| `form_narrative` | `program_version_id` | integer | no | → `program_version` |
| `form_narrative` | `evaluation_form_id` | integer | no | → `evaluation_form` |
| `form_narrative` | `prompt` | text | no |  |
| `form_narrative` | `required` | flag | no |  |
| `form_narrative` | `sort_order` | integer | no |  |
| `finalization_policy` | `program_version_id` | integer | no | key; → `program_version` |
| `finalization_policy` | `review_approved` | flag | no |  |
| `finalization_policy` | `required_narratives` | flag | no |  |
| `finalization_policy` | `ratings_complete` | flag | no |  |

#### Training

| Table | Column | Type | Null | Rule |
| --- | --- | --- | --- | --- |
| `enrollment` | `id` | integer | no | key |
| `enrollment` | `user_id` | integer | no | → `user` |
| `enrollment` | `program_version_id` | integer | no | → `program_version` |
| `enrollment` | `enrolled_at` | integer | no |  |
| `enrollment` | `enrolled_by` | integer | yes | → `user` |
| `enrollment_event` | `id` | integer | no | key |
| `enrollment_event` | `enrollment_id` | integer | no | → `enrollment` |
| `enrollment_event` | `kind` | text | no | one of `version_change`, `withdraw`, `complete`, `reinstate` |
| `enrollment_event` | `occurred_at` | integer | no |  |
| `enrollment_event` | `actor_user_id` | integer | yes | → `user` |
| `enrollment_event` | `reason` | text | no |  |
| `enrollment_event` | `from_program_version_id` | integer | yes | → `program_version` |
| `enrollment_event` | `to_program_version_id` | integer | yes | → `program_version` |
| `phase_event` | `id` | integer | no | key |
| `phase_event` | `enrollment_id` | integer | no | → `enrollment` |
| `phase_event` | `kind` | text | no | one of `advance`, `return`, `restart`, `pause`, `resume`, `complete` |
| `phase_event` | `from_phase_id` | integer | yes | → `phase` |
| `phase_event` | `to_phase_id` | integer | yes | → `phase` |
| `phase_event` | `effective_at` | integer | no |  |
| `phase_event` | `recorded_at` | integer | no |  |
| `phase_event` | `actor_user_id` | integer | yes | → `user` |
| `phase_event` | `reason` | text | no |  |
| `phase_event` | `version_change_event_id` | integer | yes | → `enrollment_event` |
| `training_assignment` | `id` | integer | no | key |
| `training_assignment` | `enrollment_id` | integer | no | → `enrollment` |
| `training_assignment` | `trainer_user_id` | integer | no | → `user` |
| `training_assignment` | `assigned_at` | integer | no |  |
| `training_assignment` | `assigned_by` | integer | yes | → `user` |
| `training_assignment` | `ended_at` | integer | yes |  |
| `training_assignment` | `ended_by` | integer | yes | → `user` |
| `training_session` | `id` | integer | no | key |
| `training_session` | `enrollment_id` | integer | no | → `enrollment` |
| `training_session` | `program_version_id` | integer | no | → `program_version` |
| `training_session` | `business_date` | text | no |  |
| `training_session` | `timezone` | text | no |  |
| `training_session` | `local_start` | text | no |  |
| `training_session` | `local_end` | text | yes |  |
| `training_session` | `utc_start` | integer | no |  |
| `training_session` | `utc_end` | integer | yes |  |
| `training_session` | `phase_id` | integer | yes | → `phase` |
| `training_session` | `disposition` | text | yes | one of `completed`, `interrupted`, `cancelled` |
| `training_session` | `created_at` | integer | no |  |
| `training_session` | `created_by` | integer | yes | → `user` |
| `training_session` | `closed_at` | integer | yes |  |
| `training_session` | `closed_by` | integer | yes | → `user` |
| `session_trainer` | `id` | integer | no | key |
| `session_trainer` | `session_id` | integer | no | → `training_session` |
| `session_trainer` | `trainer_user_id` | integer | no | → `user` |
| `session_trainer` | `added_at` | integer | no |  |
| `session_trainer` | `added_by` | integer | yes | → `user` |
| `task_signoff` | `id` | integer | no | key |
| `task_signoff` | `enrollment_id` | integer | no | → `enrollment` |
| `task_signoff` | `task_id` | integer | no | → `task` |
| `task_signoff` | `kind` | text | no | one of `observed`, `demonstrated`, `revoked` |
| `task_signoff` | `reason` | text | no |  |
| `task_signoff` | `signed_by` | integer | no | → `user` |
| `task_signoff` | `signed_by_display_name` | text | no |  |
| `task_signoff` | `signed_at` | integer | no |  |

#### Evaluations

| Table | Column | Type | Null | Rule |
| --- | --- | --- | --- | --- |
| `evaluation_record` | `id` | integer | no | key |
| `evaluation_record` | `enrollment_id` | integer | no | → `enrollment` |
| `evaluation_record` | `program_version_id` | integer | no | → `program_version` |
| `evaluation_record` | `evaluation_form_id` | integer | no | → `evaluation_form` |
| `evaluation_record` | `owner_user_id` | integer | no | → `user` |
| `evaluation_record` | `revision` | integer | no |  |
| `evaluation_record` | `created_at` | integer | no |  |
| `evaluation_record` | `created_by` | integer | yes | → `user` |
| `evaluation_session` | `id` | integer | no | key |
| `evaluation_session` | `evaluation_record_id` | integer | no | → `evaluation_record` |
| `evaluation_session` | `training_session_id` | integer | no | → `training_session` |
| `evaluation_version` | `id` | integer | no | key |
| `evaluation_version` | `evaluation_record_id` | integer | no | → `evaluation_record` |
| `evaluation_version` | `version_number` | integer | no |  |
| `evaluation_version` | `record_schema` | integer | no |  |
| `evaluation_version` | `content_hash` | text | no | 64 lowercase hex |
| `evaluation_version` | `chain_hash` | text | no | 64 lowercase hex |
| `evaluation_version` | `predecessor_id` | integer | yes | → `evaluation_version` |
| `evaluation_version` | `finalized_at` | integer | no |  |
| `evaluation_version` | `finalized_by` | integer | no | → `user` |
| `summary_daily_link` | `id` | integer | no | key |
| `summary_daily_link` | `summary_record_id` | integer | no | → `evaluation_record` |
| `summary_daily_link` | `daily_version_id` | integer | no | → `evaluation_version` |
| `draft_rating` | `id` | integer | no | key |
| `draft_rating` | `evaluation_record_id` | integer | no | → `evaluation_record` |
| `draft_rating` | `program_version_id` | integer | no | → `program_version` |
| `draft_rating` | `form_competency_id` | integer | no | → `form_competency` |
| `draft_rating` | `value` | integer | yes |  |
| `draft_rating` | `not_observed` | flag | no |  |
| `draft_rating_modifier` | `id` | integer | no | key |
| `draft_rating_modifier` | `draft_rating_id` | integer | no | → `draft_rating` |
| `draft_rating_modifier` | `program_version_id` | integer | no | → `program_version` |
| `draft_rating_modifier` | `rating_modifier_id` | integer | no | → `rating_modifier` |
| `draft_narrative` | `id` | integer | no | key |
| `draft_narrative` | `evaluation_record_id` | integer | no | → `evaluation_record` |
| `draft_narrative` | `program_version_id` | integer | no | → `program_version` |
| `draft_narrative` | `form_narrative_id` | integer | no | → `form_narrative` |
| `draft_narrative` | `text` | text | no |  |
| `draft_snapshot` | `id` | integer | no | key |
| `draft_snapshot` | `evaluation_record_id` | integer | no | → `evaluation_record` |
| `draft_snapshot` | `reason` | text | no | one of `submission`, `change_request_return` |
| `draft_snapshot` | `content` | text | no |  |
| `draft_snapshot` | `taken_at` | integer | no |  |
| `draft_snapshot` | `taken_by` | integer | yes | → `user` |
| `contributor_event` | `id` | integer | no | key |
| `contributor_event` | `evaluation_record_id` | integer | no | → `evaluation_record` |
| `contributor_event` | `kind` | text | no | one of `created`, `contributed`, `ownership_transferred`, `submitted_for_review`, `review_decided` |
| `contributor_event` | `actor_user_id` | integer | no | → `user` |
| `contributor_event` | `to_user_id` | integer | yes | → `user` |
| `contributor_event` | `recorded_at` | integer | no |  |
| `review_decision` | `id` | integer | no | key |
| `review_decision` | `evaluation_record_id` | integer | no | → `evaluation_record` |
| `review_decision` | `reviewer_user_id` | integer | no | → `user` |
| `review_decision` | `decision` | text | no | one of `approved`, `changes_requested`, `returned` |
| `review_decision` | `comment` | text | no |  |
| `review_decision` | `decided_at` | integer | no |  |
| `acknowledgment` | `id` | integer | no | key |
| `acknowledgment` | `evaluation_version_id` | integer | no | → `evaluation_version` |
| `acknowledgment` | `user_id` | integer | no | → `user` |
| `acknowledgment` | `kind` | text | no | one of `acknowledged`, `acknowledged_with_response`, `refused`, `supervisor_attested_refusal`, `unavailable` |
| `acknowledgment` | `response` | text | no |  |
| `acknowledgment` | `recorded_by` | integer | no | → `user` |
| `acknowledgment` | `user_display_name` | text | no |  |
| `acknowledgment` | `recorded_by_display_name` | text | no |  |
| `acknowledgment` | `recorded_at` | integer | no |  |
| `amendment` | `id` | integer | no | key |
| `amendment` | `evaluation_record_id` | integer | no | → `evaluation_record` |
| `amendment` | `predecessor_version_id` | integer | no | → `evaluation_version` |
| `amendment` | `reason` | text | no |  |
| `amendment` | `opened_by` | integer | no | → `user` |
| `amendment` | `opened_by_display_name` | text | no |  |
| `amendment` | `opened_at` | integer | no |  |
| `amendment` | `opened_after_event_id` | integer | no |  |
| `amendment` | `opened_after_decision_id` | integer | no |  |

#### Retention

| Table | Column | Type | Null | Rule |
| --- | --- | --- | --- | --- |
| `retention_policy` | `id` | integer | no | key |
| `retention_policy` | `record_class` | text | no | one of `daily_report`, `weekly_summary`, `phase_evaluation`, `disposition_event` |
| `retention_policy` | `version_number` | integer | no |  |
| `retention_policy` | `supersedes_id` | integer | yes | → `retention_policy` |
| `retention_policy` | `authority` | text | no |  |
| `retention_policy` | `retention_trigger` | text | no | one of `finalized_at`, `enrollment_closed_at`, `disposed_at` |
| `retention_policy` | `retention_days` | integer | no |  |
| `retention_policy` | `action` | text | no | one of `retain`, `destroy` |
| `retention_policy` | `reason` | text | no |  |
| `retention_policy` | `created_by` | integer | no | → `user` |
| `retention_policy` | `created_at` | integer | no |  |
| `record_hold` | `id` | integer | no | key |
| `record_hold` | `enrollment_id` | integer | yes | → `enrollment` |
| `record_hold` | `evaluation_record_id` | integer | yes | → `evaluation_record` |
| `record_hold` | `kind` | text | no | one of `litigation`, `anticipated_litigation`, `audit`, `investigation`, `public_records_request`, `other` |
| `record_hold` | `authority` | text | no |  |
| `record_hold` | `reason` | text | no |  |
| `record_hold` | `created_by` | integer | no | → `user` |
| `record_hold` | `created_at` | integer | no |  |
| `record_hold` | `replaces_id` | integer | yes | → `record_hold` |
| `hold_release` | `hold_id` | integer | no | key; → `record_hold` |
| `hold_release` | `released_by` | integer | no | → `user` |
| `hold_release` | `released_at` | integer | no |  |
| `hold_release` | `reason` | text | no |  |
| `hold_release` | `replacement_id` | integer | yes | → `record_hold` |

#### Notices and audit

| Table | Column | Type | Null | Rule |
| --- | --- | --- | --- | --- |
| `notice` | `id` | integer | no | key |
| `notice` | `user_id` | integer | no | → `user` |
| `notice` | `kind` | text | no | notice kind (see Vocabularies) |
| `notice` | `message` | text | no |  |
| `notice` | `created_at` | integer | no |  |
| `notice` | `read_at` | integer | yes |  |
| `audit_event` | `id` | integer | no | key |
| `audit_event` | `occurred_at` | integer | no |  |
| `audit_event` | `kind` | text | no | audit event kind (see Vocabularies) |
| `audit_event` | `actor_user_id` | integer | yes | → `user` |
| `audit_event` | `subject_user_id` | integer | yes | → `user` |
| `audit_event` | `subject_kind` | text | yes | audit subject kind (see Vocabularies) |
| `audit_event` | `subject_id` | integer | yes |  |

### References

A column marked → holds the `id` of a row in the named table (for
`capability_grant.user_id`, a `user` id). A non-null reference must name a row
the archive carries; one that does not resolve is a finding. These are the
installation's stored foreign keys. Three columns, `draft_narrative`,
`draft_rating`, and `draft_rating_modifier`'s `program_version_id`, have no
single-column foreign key of their own. Composite keys pin them to the row they
agree with (see Agreements), and they are checked as references into
`program_version` like the rest. One committed snapshot of the database
satisfies every one of these, so a reference that fails to resolve means the
archive is not the snapshot it claims to be.

`audit_event.subject_kind` and `audit_event.subject_id` are deliberately *not*
a reference. Migration 0004 gives them no foreign key, so the append-only
audit trail never blocks lawful disposition of what it describes. A subject
may name a row that no longer exists; a discarded draft program version is
one example. The verifier checks the vocabulary of `subject_kind`, and that
`subject_kind` and `subject_id` are both null or both present with a positive
id. It does not require the subject to resolve.

### Agreements

The stored schema pins configuration to a program version with composite keys:
a row's `program_version_id` must equal that of the configuration row it
references. The verifier checks the same agreements. A row whose
`program_version_id` differs from the `program_version_id` of the row named by
any column listed for it is a finding:

| Table | `program_version_id` agrees with the row named by |
| --- | --- |
| `draft_narrative` | `evaluation_record_id`, `form_narrative_id` |
| `draft_rating` | `evaluation_record_id`, `form_competency_id` |
| `draft_rating_modifier` | `draft_rating_id`, `rating_modifier_id` |
| `evaluation_record` | `evaluation_form_id` |
| `form_competency` | `evaluation_form_id`, `competency_id`, `rating_scale_id` |
| `form_narrative` | `evaluation_form_id` |
| `phase_transition` | `from_phase_id`, `to_phase_id` |
| `rating_anchor` | `rating_scale_id` |
| `standards_citation` | `competency_id`, `task_id` (when not `null`) |
| `task` | `competency_id` |
| `training_session` | `phase_id` (when not `null`) |

### Vocabularies

Columns marked "one of" carry the vocabulary the stored table's `CHECK`
constraint names. Four columns draw from a vocabulary the owning module
defines instead, because the table has no `CHECK`:

- **capability** (`capabilities.rs`): `manage_retention`, `manage_users`,
  `manage_programs`, `assign_training`, `export_records`, `author_evaluation`,
  `review_evaluation`, `view_assigned_records`, `view_own_records`,
  `acknowledge_own_record`.
- **notice kind** (`notices.rs`): `backup_failed`, `backup_copy_failed`,
  `assignment_created`, `draft_ownership_received`,
  `draft_submitted_for_review`, `draft_approved`, `draft_changes_requested`,
  `draft_returned`, `draft_finalized`, `record_awaits_acknowledgment`,
  `acknowledgment_response`, `acknowledgment_refused`,
  `acknowledgment_attested`, `amendment_opened`.
- **audit event kind** (`audit.rs`): `retention_authority_granted`,
  `retention_authority_revoked`, `retention_policy_created`,
  `record_hold_created`, `record_hold_replaced`, `record_hold_released`,
  `setup_completed`, `login_succeeded`, `login_failed`, `logout`,
  `reset_code_issued`, `reset_code_used`, `recovery_code_issued`,
  `backup_completed`, `restore_completed`, `program_created`,
  `program_version_created`, `program_version_published`,
  `program_version_imported`, `program_version_discarded`, `user_created`,
  `enrollment_created`, `enrollment_withdrawn`, `enrollment_completed`,
  `enrollment_reinstated`, `enrollment_version_changed`,
  `phase_event_recorded`, `assignment_created`, `assignment_ended`,
  `session_created`, `session_updated`, `session_closed`,
  `session_trainer_added`, `session_trainer_removed`, `draft_created`,
  `draft_ownership_transferred`, `draft_submitted`, `draft_review_decided`,
  `draft_finalized`, `acknowledgment_recorded`, `amendment_opened`,
  `task_signoff_recorded`, `record_exported`, `trainee_packet_exported`,
  and `installation_exported` (see Authorization).
- **audit subject kind** (`audit.rs`): `retention_policy`, `record_hold`,
  `program`, `program_version`, `enrollment`, `assignment`, `session` (a
  training session), and `record` (an evaluation record).

Every vocabulary is closed and is shared with the module that owns it. The
producer parses each stored value on the way out. The verifier parses it on
the way in. A value outside the vocabulary is refused at production, as a
stored value the build does not understand, and is a finding at verification.
It is never passed through as text. Kinds are only ever added, never removed,
because an older installation's history must keep exporting.

## What the export does not carry

- **Credentials:** the `session`, `session_secure_cookie`,
  `password_reset_code`, and `setup_code` tables, and `user.password_hash`.
  The ledger names each one. Issuing and using reset and recovery codes, and
  signing in and out, still appear in the audit trail as events, without the
  secrets.
- **Schema rather than data:** indexes, triggers, the
  `evaluation_record_frozen` view, and the migration framework's bookkeeping
  table (`_sqlx_migrations`). The format's columns and rules are the schema a
  reader needs. The coverage guard (below) ignores only `_sqlx_migrations` and
  SQLite's own `sqlite_` tables, by name.
- **Files outside the database:** backups and backup copies (the export is not
  a backup and cannot be restored into an installation; see ADR 0006 and ADR
  0024), the service configuration, and the serve lock.
- **Exporter identity:** the audit trail records it (`installation_exported`).
- **Signatures, rendered PDFs, and attachments:** the same as for the record
  export. Signatures and PDFs arrive under later decisions. No stored
  attachment exists yet.

The archive holds the personal data of every person the installation knows,
along with drafts, notices, and the audit trail. Like a backup copy (ADR 0027),
it is not encrypted. Store and transfer it as the installation's own data.

## Determinism and consistency

An export is a pure function of the stored rows it reads and the export
instant. The same rows exported at the same instant produce identical bytes.
Exporting changes the installation, though: each export records its own
`installation_exported` audit event before it reads (see Authorization), so the
archive carries the event of its own production. Two successive exports
therefore always differ, at least by that row and its table's ledger entry.

The producing installation reads the unit list, every table document, and the
ledger counts inside one read transaction, so every document describes one
committed state. A row that references another can therefore never arrive
without it, and a count never disagrees with its document. Unit bytes are then
written as the record export writes them. Each unit is checked against the
stored hashes the manifest already states. Finalized versions are immutable,
so a unit that no longer matches ends the transfer incomplete rather than
producing a different archive. The record export's delivery rules hold: no
database transaction is held across the transfer, and an incomplete transfer
leaves an unreadable file, not a smaller valid one.

## Verification

The record export's verifier reads the root manifest's `format` and applies
these checks when it names an installation export. The verdict keeps the
same honest meaning: the archive is consistent with its stated fingerprints
and its format. It is not tamper-proof against whoever produced the file, and
it is not proof of which installation produced it.

The verifier checks the shape this document names: members, types,
nullability, order, keys, closed vocabularies, references, agreements, counts,
and the unit lineage. It does not re-run every stored constraint (text
lengths, cross-member rules such as an acknowledgment's speaker), which the
database enforced when the rows were written. Those constraints keep their
authority in the schema. The verifier makes no claim about them.

Archive checks:

1. the container checks of the record export (a readable ZIP, no name written
   twice, nothing unlisted);
2. `manifest.json` parses and carries this `format` and a known
   `format_version`;
3. the ledger is exactly the format version's closed set, with each name once,
   ascending, with known reasons, each carried table at its derived path;
4. `units` are listed as the record export lists them (ascending, no duplicate
   identity, derived paths), and every unit's predecessor is carried;
5. `tables/instance.json` holds exactly one row, and its `installation_id`
   equals the manifest's.

Unit checks are exactly the record export's.

Document checks, for every carried table:

1. the document exists, and the SHA-256 of its bytes equals the manifest's;
2. the bytes are canonical JSON, and they form an array of row objects with
   exactly the table's members, typed, and with nullability and closed
   vocabularies as listed;
3. rows are strictly ascending by the table's key, and the row count equals
   the manifest's `rows`;
4. every reference resolves (References) and every agreement holds
   (Agreements);
5. the `evaluation_version` rows and the units agree both ways (Units).

`consolebook-server export verify <archive>` prints one line per unit and per
table document, then every finding. It exits non-zero unless the verdict is
`verified`, and it opens no data directory.

## Authorization and delivery

Only holders of `export_records` may produce an installation export. That is
the capability that already exports every finalized record installation-wide
(ADR 0010, ADR 0014). This export also carries people, drafts, and the audit
trail, so no narrower read rule could produce it. The interface offers it on
the home page beside the installation record export, at
`GET /api/exports/installation`, as
`consolebook-installation-export-{stamp}.zip`. Every export is audited as
`installation_exported`, with the actor and never with content. The audit
event is committed before the snapshot is read, so the archive includes it. The audit
event records the export the installation produced from its state at the
recorded instant. As with the record export, it is not a receipt for a
completed download. The event stands for the attempt: an export refused at
production, for example because a stored value is one this build does not
know, still leaves its `installation_exported` row, as the record export's
does.

## Coverage and format versions

The format's table and column lists are enforced, not just written down:

- A test migrates a fresh database and compares every table and column in it
  (excluding `sqlite_` tables and `_sqlx_migrations`) with the producer's
  table specification. A table or column that is neither carried nor excluded
  fails the test, as does a specification entry the schema no longer has.
  A migration that adds a table or column therefore cannot land without
  deciding how this format carries it.
- A second test reads the column tables in this document and compares them
  with the same specification, so the document cannot drift from the
  producer.
- A third test pins a fingerprint of the specification (every table, key,
  column rule, exclusion, and column carried as units) to each
  `format_version`. A changed specification fails it until `format_version`
  is bumped and the new version's fingerprint is recorded beside the earlier
  ones; a recorded fingerprint is never edited. This test is what enforces
  the bump.

Changing what the format carries changes the format: a new `format_version`,
this document, the producer, the verifier, and the fixtures move together
(CONTRIBUTING.md). A build refuses an archive whose `format_version` it does
not know, and says so by name. The ADR for any later version decides how
earlier archives keep verifying.
