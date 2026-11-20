# Portable Disposition Evidence Format

Minimal, allowlisted portable evidence for lawfully disposed evaluation records
and policy-required metadata boundary checkpoints ([ADR 0030](../decisions/0030-lawful-disposition-scope-and-recovery.md),
[#212](https://github.com/FieldmouseWorks/consolebook/issues/212),
[#64](https://github.com/FieldmouseWorks/consolebook/issues/64)).

`crates/consolebook-server/src/disposition_evidence.rs` defines the closed
vocabularies and canonical producer, and
`crates/consolebook-server/src/disposition_evidence/verify.rs` verifies a
document from its bytes alone.

## Authenticity and non-guarantees

This format carries `"authenticity": "self_consistency_only"`. Verification
proves only that the document is canonical JSON matching version 1's allowlisted
schema and that any retained version hashes form an internally continuous
`1..=N` predecessor chain.

- It **never** claims that destroyed record content was verified.
- It **never** claims cryptographic signatures, external timestamping, or
  tamper-proofing that Consolebook does not implement.
- Any document claiming another `authenticity` value or adding an unknown field
  (such as `signature`, `narrative`, `display_name`, `reason`, or `response`) is
  refused with a typed finding.

## Encoding

The document is one canonical JSON object under Consolebook's RFC 8785 subset
(`docs/records-integrity.md`, `src/canonical.rs`): UTF-8, ASCII keys sorted by
code point, compact formatting, integers within `-(2^53 - 1)..=(2^53 - 1)`.

## Root members (`format_version: 1`)

| Member | Type | Rule |
| --- | --- | --- |
| `format` | string | Always `"consolebook-disposition-evidence"` |
| `format_version` | integer | `1`; unknown versions are refused by name |
| `installation_id` | string | Non-blank exporting installation identifier |
| `exported_at` | integer | UTC Unix seconds |
| `authenticity` | string | Always `"self_consistency_only"` |
| `dispositions` | array | Minimal tombstones, strictly ascending by `evaluation_record_id` |
| `boundary_checkpoints` | array | Policy-required metadata closures, strictly ascending by `checkpoint_id` |

## Disposition entries (`dispositions[]`)

Each entry carries only opaque identifiers, closed vocabulary codes, a bounded
agency authority reference, UTC timestamps, and optional version hashes:

| Member | Type | Rule |
| --- | --- | --- |
| `evaluation_record_id` | integer | Positive, unique, strictly ascending |
| `enrollment_id` | integer | Positive |
| `record_class` | string | One of `daily_report`, `weekly_summary`, `phase_evaluation` |
| `record_policy_id` | integer | Positive; evaluation-class `destroy` policy that authorized disposition |
| `metadata_policy_id` | integer or null | Positive when present; independent `disposition_event` policy |
| `version_count` | integer | Positive |
| `earliest_finalized_at` | integer | UTC Unix seconds |
| `latest_finalized_at` | integer | UTC Unix seconds, `>= earliest_finalized_at` |
| `hashes_retained` | boolean | `true` iff `versions` carries `1..=version_count` continuous hashes |
| `authority_reference` | string | 1–200 non-blank characters (agency schedule/authority reference) |
| `reason_code` | string | One of `scheduled_retention_elapsed`, `restore_ledger_reconciliation` |
| `method` | string | One of `lawful_destruction`, `restore_reconciliation` |
| `outcome` | string | Always `disposed` |
| `disposed_at` | integer | UTC Unix seconds, `>= latest_finalized_at` |
| `versions` | array | Empty when `hashes_retained` is `false`; exactly `version_count` entries `1..=version_count` when `true` |

Each `versions[]` entry carries `version_number`, `record_schema`,
`content_hash`, `chain_hash`, `predecessor_content_hash` (`null` for version 1,
previous version's `content_hash` for versions `> 1`), and `finalized_at`.

### Verifier verdicts for disposition entries

- `disposed_hash_continuity_only`: `hashes_retained` is `true` and the
  `1..=version_count` predecessor hash chain is internally continuous, while
  record content is lawfully disposed and unavailable for content verification.
- `disposed_content_and_hashes_unavailable`: `hashes_retained` is `false` and
  `versions` is empty; neither content nor version hashes are retained.

## Boundary checkpoints (`boundary_checkpoints[]`)

Each checkpoint records an intentional policy-required metadata closure after
expired disposition tombstones are removed (`#218`):

| Member | Type | Rule |
| --- | --- | --- |
| `checkpoint_id` | integer | Positive, unique, strictly ascending |
| `metadata_policy_id` | integer | Positive; `disposition_event` `destroy` policy |
| `authority_reference` | string | 1–200 non-blank characters |
| `reason_code` | string | Always `metadata_retention_elapsed` |
| `expired_event_count` | integer | Positive |
| `boundary_through_disposed_at` | integer | UTC Unix seconds |
| `recorded_at` | integer | UTC Unix seconds, `>= boundary_through_disposed_at` |

### Verifier verdict for boundary checkpoints

- `intentional_policy_boundary`: marks lawful policy-required metadata closure
  through `boundary_through_disposed_at` — never silent omission and never
  content verification.
