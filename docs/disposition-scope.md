# Lawful Disposition Scope and Recovery Specification

This specification is the source-linked inventory accompanying
[ADR 0030](decisions/0030-lawful-disposition-scope-and-recovery.md)
([#210](https://github.com/FieldmouseWorks/consolebook/issues/210),
[#64](https://github.com/FieldmouseWorks/consolebook/issues/64)). It maps every
current SQLite table, trigger, index, and filesystem location in Consolebook to
its disposition treatment, dependency rule, and downstream test requirement
(`#211`–`#221`).

> **Implementation status:** Policy and hold administration are implemented
> ([ADR 0020](decisions/0020-retention-policy-and-hold-administration.md),
> [retention.md](retention.md)). **No runtime disposition execution exists yet.**
> This document defines the contract that `#211`–`#219` must implement and prove.

## 1. SQLite content and lineage inventory

Verified against `crates/consolebook-server/migrations/0001_instance.sql`
through `0016_session_cookie_secure.sql` and
`crates/consolebook-server/src/installation_export/spec/tables.rs` (43 carried
tables).

### A. Destroyed with a disposed `evaluation_record` lineage

All rows below belong to the disposed record's lineage and are deleted inside
one atomic `BEGIN IMMEDIATE` transaction once confirmation checks pass, leaving
no authored ratings, narratives, snapshots, responses, or amendment reasons in
the live database.

| Artifact | Owning migration & table | Content / PII carried | Existing guards to preserve for ordinary writes | Disposition treatment | Required test proof (downstream slice) |
| --- | --- | --- | --- | --- | --- |
| Canonical finalized versions | `0010` `evaluation_version` (`evaluation_version_record` index) | `canonical_bytes` BLOB (full RFC 8785 envelope: ratings, narratives, presentation names, employee ID, title, review history, attribution), `content_hash`, `chain_hash`, `predecessor_id` | `evaluation_version_no_update`, `evaluation_version_no_delete` | Delete all versions `1..=N` of the record lineage after copying opaque fingerprints/timestamps into `disposition_event` (when permitted) and recovery ledger. | `#214`: raw `DELETE` still fails closed; authorized disposition removes every version; partial-version deletion refused. |
| Acknowledgments | `0011` `acknowledgment` (`acknowledgment_user` index) | `response` (trainee statement, refusal reason, supervisor attestation), `user_display_name`, `recorded_by_display_name` | `acknowledgment_no_update`, `acknowledgment_no_delete`, FK `evaluation_version_id` | Delete every acknowledgment bound to any version of the disposed record before deleting `evaluation_version`. | `#214`: every version's acknowledgments removed; raw `DELETE` still fails closed. |
| Amendments | `0012` `amendment` (`amendment_record` index) | `reason` (free-text explanation), `opened_by_display_name`, high-water marks | `amendment_no_update`, `amendment_no_delete`, FKs `evaluation_record_id`, `predecessor_version_id` | Refuse disposition if an amendment is open (targets latest version). Otherwise delete all closed amendment rows before deleting `evaluation_version`. | `#211`/`#214`: open amendment refused; closed amendment rows removed with lineage. |
| Working-copy ratings & modifiers | `0008`/`0010` `draft_rating`, `draft_rating_modifier` | Last authored `value`, `not_observed`, modifier links | `draft_rating_frozen_*`, `draft_rating_modifier_frozen_*` (`0009`) | Delete `draft_rating_modifier` then `draft_rating` for the record inside the disposition-gated transaction. | `#214`: frozen working-copy rows removed only under disposition context; ordinary edits/deletes remain frozen. |
| Working-copy narratives | `0008` `draft_narrative` | Last authored free-text `text` | `draft_narrative_frozen_*` (`0009`) | Delete all `draft_narrative` rows for the record. | `#214`: narrative rows removed; ordinary deletes remain frozen. |
| Review content snapshots | `0008` `draft_snapshot` (`draft_snapshot_record` index) | Full JSON `content` copy of ratings & narratives at `submission` and `change_request_return` | `draft_snapshot_no_update`, `draft_snapshot_no_delete` | Delete all `draft_snapshot` rows for the record. | `#214`: submission and change-request snapshots removed; raw `DELETE` still fails closed. |
| Review decisions | `0009` `review_decision` (`review_decision_record` index) | `decision`, free-text `comment` | `review_decision_no_update`, `review_decision_no_delete` | Delete all `review_decision` rows for the record. | `#214`: review comments removed; raw `DELETE` still fails closed. |
| Contributor attribution stream | `0008` `contributor_event` (`contributor_event_record` index) | `kind`, `actor_user_id`, `to_user_id`, `recorded_at` | `contributor_event_no_update`, `contributor_event_no_delete` | Delete all `contributor_event` rows for the record. | `#214`: attribution rows removed with lineage; raw `DELETE` still fails closed. |
| Record–session links | `0008` `evaluation_session` (`evaluation_session_session` index) | Link between `evaluation_record_id` and `training_session_id` | `evaluation_session_frozen_delete` (`0009`) | Delete `evaluation_session` rows for the disposed record; `training_session` itself stays intact. | `#214`: link removed without deleting `training_session`. |
| Weekly-summary links | `0013` `summary_daily_link` | `summary_record_id`, `daily_version_id` | `summary_daily_link_frozen_delete`, FK `daily_version_id -> evaluation_version(id)` | When disposing a weekly summary, delete its outgoing `summary_daily_link` rows. When disposing a daily report, **refuse** if any retained summary outside the confirmed batch still links any version of it (Section 2). | `#211`/`#214`: daily report with retained dependent summary refuses (`DependentSummaryRetained`); explicit joint batch succeeds; summary-only disposition leaves linked dailies intact. |
| Record-linked workflow notices | `0003` `notice` (`notice_user_unread` index); forward migration adds nullable `evaluation_record_id` | Generic workflow sentence + trainee display name (`src/evaluation_drafts.rs`, `src/draft_review.rs`, `src/finalization.rs`, `src/amendments.rs`, `src/acknowledgments.rs`); zero narratives or ratings | Recipient-scoped reads (`notices.rs`) | Delete `notice` rows carrying the disposed `evaluation_record_id`. Pre-migration rows without `evaluation_record_id` are not substring-matched; documented as an explicit legacy boundary (Section 6). | `#214`: newly created record-linked notices are removed on disposition; unrelated notices survive. |
| Record identity row | `0008` `evaluation_record` (`evaluation_record_enrollment` index) | `id`, `enrollment_id`, `program_version_id`, `evaluation_form_id`, `owner_user_id`, `created_by`, `created_at` | `evaluation_record_keeps_identity`, referenced by FKs above and by `record_hold` | Delete `evaluation_record` last in the DB transaction (after decoupling `record_hold`'s blocking FK via forward migration; Section 2). | `#214`: `evaluation_record` removed cleanly; no orphaned child rows; `PRAGMA foreign_key_check` empty. |
| Future attachments (reserved) | `docs/records-integrity.md` (`canonical_bytes.attachments = []` today) | None implemented today | — | Any future attachment metadata table and filesystem blob store must register with the disposition transaction and copy-cleanup phase before attachments ship. | Future attachment slice must extend `#214` tests before landing. |

### B. Retained outside record content (must NOT carry destroyed content)

| Artifact | Owning migration & table | Disposition treatment | Required test proof |
| --- | --- | --- | --- |
| Audit trail | `0002`/`0004` `audit_event` (`src/audit.rs`) | Append-only, no foreign key to `evaluation_record` (`subject_kind = 'record'`, `subject_id` opaque), zero narratives/ratings/reasons. Retained intact; disposition appends typed `disposition_*` audit events with opaque IDs only. | `#214`: no destroyed narrative, response, or reason appears in `audit_event` or logs. |
| Retention policies & authority events | `0015` `retention_policy`, `retention_authority_event` | Append-only configuration/authority history. Never modified or deleted by record disposition; extended in `#211` to distinguish `manage_retention` and `execute_disposition`. | `#211`/`#214`: policies and authority events remain immutable and referenced by tombstones. |
| Record holds & releases | `0015` `record_hold`, `hold_release` | Append-only preservation history. Any **active** hold (installation, enrollment, or record scope) blocks disposition. **Released** record-scoped holds must not block disposition once migration replaces `record_hold.evaluation_record_id`'s blocking FK with an insert-time trigger (Section 2). | `#211`/`#214`: active hold in any of the 3 scopes and 6 kinds blocks; released hold allows disposition while hold/release history stays intact. |
| Enrollment, sessions, signoffs, users, programs | `0002`, `0004`–`0007`, `0013`–`0014` (`user`, `program*`, `enrollment*`, `phase_event`, `training_assignment`, `training_session`, `task_signoff`) | Out of scope for evaluation-record disposition; untouched. (`enrollment_event` supplies `enrollment_closed_at` when latest status event is `complete` or `withdraw`.) | `#214`: enrollment, sessions, and `task_signoff` rows survive disposition of an evaluation record untouched. |
| New disposition tables (`#212`/`#214`) | `disposition_run`, `disposition_event`, `disposition_version_hash`, `disposition_boundary_checkpoint` | Minimal tombstones and run-recovery state (Section 4). Governed independently by the `disposition_event` policy class (`retention_trigger = 'disposed_at'`). | `#212`/`#214`/`#218`: zero destroyed personal text; independent expiry removes expired tombstones and leaves a policy-boundary checkpoint. |

## 2. Dependency closure vs. refusal rules

1. **No silent broadening:** An operator selects explicit `evaluation_record` IDs.
   Preview and execution never silently add another record (for example, a
   dependent weekly summary or another record in the same enrollment) to the
   destruction set.
2. **Weekly summary ↔ daily report (`summary_daily_link`):**
   - Disposing daily report `D` while any retained `weekly_summary` `S` links
     any version of `D` in `summary_daily_link`:
     - If `S` is **not** in the operator's selected set → **refuse**
       (`DependentSummaryRetained { record_id: D, dependent_summary_ids: [S, ...] }`).
     - If `S` **is** in the selected set, is finalized with no open amendment,
       is eligible under its own `weekly_summary` policy, and has no active
       hold → **allow**; delete `summary_daily_link` rows for `S` first, then
       dispose both `S` and `D` in the same transaction.
     - If `S` is in the selected set but is unfinalized, has an open amendment,
       is ineligible, or is held → **refuse** the entire batch (fail closed; no
       partial batch execution).
   - Disposing `weekly_summary` `S` without `D` → **allow** (if `S` is eligible
     and unheld); delete `S`'s `summary_daily_link` rows and `S`'s lineage;
     `D` stays untouched.
3. **Open amendment:** A finalized record whose latest `evaluation_version` has
   a matching `amendment.predecessor_version_id` is actively reopened and
   **refused** (`OpenAmendmentBlocking`).
4. **Released `record_hold` FK decoupling:** Migration `0015` line 40 defines
   `evaluation_record_id INTEGER REFERENCES evaluation_record(id)`. Because
   `record_hold` is append-only (`record_hold_no_update`, `record_hold_no_delete`),
   a released hold would otherwise block deleting `evaluation_record` under
   `PRAGMA foreign_keys = ON`. The disposition schema migration must rebuild
   `record_hold` without the `REFERENCES evaluation_record(id)` constraint and
   add a `BEFORE INSERT` trigger asserting that `NEW.evaluation_record_id`
   exists in `evaluation_record` at hold creation time (matching `audit_event`'s
   non-blocking reference rule).

## 3. Filesystem, SQLite residual bytes, and backup-copy inventory

Verified against `crates/consolebook-server/src/data_dir.rs`, `src/backup.rs`,
`src/backup_copy.rs`, `src/restore.rs`, `src/exports_http.rs`, and
[ADR 0027](decisions/0027-backup-copy-encryption.md) (copies are plain SQLite
files, not encrypted by Consolebook).

| Location / pattern | Owner in `src/` | Application-controlled? | Disposition treatment | Required test proof |
| --- | --- | --- | --- | --- |
| `data/consolebook.db`, `-wal`, `-shm` | `storage.rs`, `data_dir.rs` | Yes | After atomic DB commit: `PRAGMA wal_checkpoint(TRUNCATE)` → `VACUUM` → `PRAGMA wal_checkpoint(TRUNCATE)` to eliminate freelist pages, B-tree slack bytes, and WAL frames containing destroyed text. | `#214`: raw byte scan of `consolebook.db`, `-wal`, and `-shm` finds zero sentinel substrings from destroyed narratives, ratings, snapshots, responses, and amendment reasons. |
| `data/backups/consolebook-<stamp>.db` | `backup.rs` (`SCHEDULED_PREFIX`) | Yes | After clean post-disposition snapshot succeeds, delete all snapshots taken prior to the clean snapshot. | `#213`/`#214`: pre-disposition local snapshots removed; post-disposition snapshot validates and passes byte scan. |
| `data/backups/consolebook-premigration-<stamp>.db` | `backup.rs` (`PRE_MIGRATION_PREFIX`) | Yes | Same: delete all pre-disposition pre-migration snapshots once the clean post-disposition snapshot is durable. | `#214`: pre-migration snapshots predating disposition are removed. |
| `data/backups/consolebook-prerestore-<stamp>.db` & `.damaged` | `backup.rs` (`PRE_RESTORE_PREFIX`), `restore.rs:61` | Yes | Same: delete pre-disposition pre-restore snapshots and `.damaged` raw files once the clean post-disposition snapshot is durable (these hold pre-disposition DB bytes and are never pruned by ordinary scheduled retention). | `#214`: both `.db` and `.damaged` pre-restore files predating disposition are removed. |
| `data/backups/*.partial`, `*.tmp` | `backup.rs` | Yes | Remove stale pre-disposition temporary files during copy cleanup. | `#214`: leftover temp files removed. |
| `CONSOLEBOOK_BACKUP_COPY_DIR/consolebook-*.db` (and `.partial`) | `backup_copy.rs` | Yes (when configured and reachable) | Copy the fresh post-disposition snapshot first; verify SHA-256; mirror the updated recovery ledger; then delete all pre-disposition copies (`consolebook-*.db`, `consolebook-premigration-*.db`, stale `.partial`). If the copy dir is configured but unreachable or delete fails, **stay in `copy_cleanup_pending`** and report incomplete. | `#213`/`#214`: reachable copy dir cleaned and verified; unreachable share keeps run in `copy_cleanup_pending` and surfaces in `doctor`/status until retried when reachable. |
| `data/exports/` | `data_dir.rs`, `exports_http.rs` | Yes | Currently unused (exports stream directly to HTTP). Any future staged/cached exports or generated PDFs (`#222`/`#223`) referencing disposed records must be deleted in copy cleanup. | `#214`/`#223`: staged export/PDF artifacts cleaned if introduced. |
| `data/instance/disposition-ledger.json` (new in `#213`) | `data_dir.rs`, `restore.rs`, `retention/` | Yes | Minimal, non-sensitive resurrection-prevention ledger (`installation_id`, `disposition_run_id`, disposed `record_id`s, disposed `content_hash`es, `disposed_at`, `policy_authority`). Atomically written + fsynced before copy deletion, mirrored to `CONSOLEBOOK_BACKUP_COPY_DIR`. | `#213`/`#221`: `restore` and `restore --verify` consult ledger and refuse silent resurrection of pre-disposition snapshots. |
| Downloaded exports & trainee packets (user devices) | None (browser download) | **No** | Cannot be deleted by Consolebook. Preview, confirmation, and operator docs state this explicitly. | `#219`: UI preview/confirmation surfaces the external-copy boundary. |
| External backups, SAN/NAS/ZFS/btrfs snapshots, SSD flash blocks | Host / infrastructure | **No** | Outside Consolebook's control. `VACUUM` cleans live SQLite/WAL bytes only; operator docs state storage-layer limits plainly. | `#224`: operator guide states exact limits without overclaiming. |

## 4. Exact confirmation invalidation and crash/retry states

### Confirmation digest inputs (`#211`, `#214`)

Preview computes `confirmation_digest = SHA-256(canonical_json(PreviewScope))`
over:

- `installation_id`
- sorted list of target items: `(record_id, record_class, enrollment_id,
  version_numbers, version_content_hashes, trigger_kind, trigger_instant,
  policy_id)`
- current `retention_policy.id` for every affected evaluation class and for
  `disposition_event`
- active hold state watermark: `(max_hold_id, max_release_id)`
- summary-link watermark: `max_summary_daily_link_id` (or exact sorted links
  within the confirmed batch)
- configured copy-destination identity (`copy_dir` path or `null`)
- `previewed_at` UTC second and `expires_at = previewed_at + 900` (15 minutes)

Any change to permissions, policies, holds, amendments, summary links, target
record versions, copy configuration, or expiry between preview and execution
invalidates the digest inside `BEGIN IMMEDIATE` and returns a typed refusal
with zero side effects.

### Persisted run state machine (`disposition_run`, `#214`)

```text
[preview confirmed]
        │
        ▼ (atomic BEGIN IMMEDIATE transaction)
  db_committed_vacuum_pending
        │  - DB rows deleted, tombstones inserted, audit_event appended
        │  - crash before commit -> full rollback (no run row exists)
        ▼ (wal_checkpoint(TRUNCATE) -> VACUUM -> wal_checkpoint(TRUNCATE))
  copy_cleanup_pending
        │  - recovery ledger written & fsynced (data/instance/)
        │  - fresh clean snapshot taken (+ copied off-server if configured)
        │  - pre-disposition local snapshots, pre-restore files, & off-server copies deleted
        │  - any failure stays in this state with per-target error detail; idempotently retryable
        ▼ (all controlled locations verified clean)
  completed
```

- **No silent completion:** Only `completed` reports disposition finished.
  `db_committed_vacuum_pending` and `copy_cleanup_pending` surface as explicit
  warnings/failures in `doctor`, `/api/backups/status`, and the retention UI
  until retried to completion.
- **No double-deletion hazard:** Retry checks the persisted `disposition_run`
  state and resumes only the unfinished post-commit steps (`VACUUM`, ledger
  sync, clean snapshot, pre-disposition copy deletion).

## 5. Portable disposition evidence and export verification (`#212`, `#215`–`#218`)

- **Record exports (`#215`, ADR 0014):** Exporting a disposed record (while its
  tombstone is retained) or an archive spanning disposed records carries
  structured disposition entries beside retained units rather than silently
  omitting them; `export verify` reports retained units verified and disposed
  units as **lawfully disposed (content unavailable for verification)**.
- **Trainee packets (`#216`, ADR 0015):** Packets carry retained units,
  retained tombstones for disposed records of that enrollment, and any
  policy-boundary closures after tombstone expiry; `export verify` never claims
  a disposed record's bytes were verified.
- **Installation exports (`#217`, ADR 0028):** Extend `installation_export/spec/tables.rs`
  (with a bumped installation format version) to carry `disposition_run`,
  `disposition_event`, `disposition_version_hash`, and
  `disposition_boundary_checkpoint`, with verifier checks confirming no
  disposed `evaluation_record_id` coexists with live `evaluation_version` rows
  and reporting disposed/closed boundaries honestly.
- **Independent tombstone expiry (`#218`):** Governed by the
  `disposition_event` policy class (`disposed_at` trigger, `destroy` action,
  no active installation hold, explicit `execute_disposition` confirmation).
  Removes expired `disposition_event` and `disposition_version_hash` rows (and
  prunes matching ledger entries only once no pre-disposition application
  snapshots remain), inserting a non-personal `disposition_boundary_checkpoint`
  row recording only the policy authority, boundary instant, and count so
  verifiers report an intentional policy boundary.

## 6. Explicit legacy and external boundaries

1. **Legacy unlinked notices (`0003_notices.sql`):** Notices created before the
   disposition migration have no `evaluation_record_id`. They contain no
   evaluation ratings, narratives, or responses, only transient workflow alerts.
   Disposition will not attempt fragile text-matching deletions against them.
2. **Unconfigured off-server copies:** If `CONSOLEBOOK_BACKUP_COPY_DIR` is not
   configured, copy cleanup covers `data/backups/` and `data/exports/` only and
   records that no off-server copy directory was configured.
3. **Clean-server restore without ledger:** If an agency loses both its server
   and its recovery ledger mirror and restores an archived pre-disposition
   snapshot onto a fresh machine, Consolebook cannot detect the prior
   disposition and states plainly in `restore` output that no recovery ledger
   was present.
4. **No supplied schedules or legal advice:** Agencies supply their own
   approved authority references, retention periods, and disposition decisions.
