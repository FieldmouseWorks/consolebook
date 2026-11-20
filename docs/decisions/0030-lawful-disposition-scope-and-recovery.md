# ADR 0030: Lawful disposition scope, authority, and recovery contract

- **Status:** Proposed (awaits owner acceptance)
- **Date:** 2026-11-20
- **Issues:** [#210](https://github.com/FieldmouseWorks/consolebook/issues/210),
  [#64](https://github.com/FieldmouseWorks/consolebook/issues/64)
- **Implements part of:** [Milestone 5 decisions](https://github.com/FieldmouseWorks/consolebook/issues/44)
- **Extends:** [ADR 0020](0020-retention-policy-and-hold-administration.md)
  (policy and hold administration),
  [ADR 0006](0006-backup-scheduling-and-restore.md) (backups and restore),
  [ADR 0011](0011-canonical-record-format-and-finalization.md) (immutability while retained),
  [ADR 0014](0014-record-export-format.md),
  [ADR 0015](0015-trainee-packet.md),
  [ADR 0024](0024-off-server-backup-copies.md),
  [ADR 0028](0028-installation-export.md)

## Context

[ADR 0020](0020-retention-policy-and-hold-administration.md) delivered versioned
retention policies, attributed holds across installation/enrollment/record
scopes, and explicit `manage_retention` authority, deliberately without a
destruction path. Before any runtime deletion code is written (`#211`–`#219`),
the system needs one reviewed contract governing:

1. **Where evaluation content actually lives** across SQLite tables, indexes,
   WAL/freelist bytes, local snapshots, pre-restore files, and configured
   off-server backup copies — and where Consolebook has **no** control
   (downloaded exports, external storage, host filesystem snapshots).
2. **How dependencies behave** (especially `summary_daily_link`,
   `record_hold`, `evaluation_version` lineage, and open amendments) without
   silently broadening an operator's confirmed selection.
3. **How authorization, exact-scope confirmation, crash recovery, tombstones,
   and restore** prevent accidental destruction, partial-state lies, and silent
   resurrection of disposed records.

The complete source-linked inventory of every table, trigger, index, and file
path lives in [docs/disposition-scope.md](../disposition-scope.md). This ADR
fixes the durable decisions that `#211`–`#221` implement. **No runtime
destruction ships with this ADR.**

## Decision

### 1. Separate destruction authority (`execute_disposition`)

Lawful disposition requires a new explicit capability, `execute_disposition`,
distinct from both `manage_retention` (policy/hold administration) and
`manage_users` (grant administration):

- No role bundle (`Administrator`, `Coordinator`, `Trainer`, `Trainee`) ever
  includes `execute_disposition`.
- Only a `manage_users` holder may grant or revoke `execute_disposition` for an
  existing user, with a non-blank reason, committed atomically with an
  attributed authority event and an `audit_event` (extending the ADR 0020
  authority model with an explicit capability discriminator).
- Holding `manage_retention` alone cannot preview-confirm or execute
  disposition; holding `execute_disposition` alone cannot revise retention
  policies or place/release holds. Agencies may separate those duties across
  two operators or explicitly grant both to one person.
- Every disposition preview, execution, retry, and tombstone-expiry service
  checks `execute_disposition` inside the same database transaction as the
  governed reads and writes (ADR 0010, ADR 0019).

### 2. Disposition unit: complete, closed-cycle record lineage

Disposition operates on a **complete `evaluation_record` lineage** — never a
single version within a multi-version amendment chain and never an unfinalized
or open-amendment draft:

- **Eligible targets** are finalized `evaluation_record` rows whose latest
  cycle is closed (at least one `evaluation_version` exists, and no
  `amendment` targets the latest `evaluation_version`), whose class has a
  current `retention_policy` with `action = 'destroy'`, whose elapsed
  24-hour-day retention period (`retention_days * 86_400` UTC seconds) has
  passed since the policy's trigger instant, and that have **zero** active
  applicable holds across installation, enrollment, and record scopes (all six
  closed hold kinds).
- **Trigger resolution:**
  - `finalized_at`: `MAX(evaluation_version.finalized_at)` for the record's
    lineage (the latest sealed version's instant, so an amendment resets the
    clock from the latest sealed record rather than disposing an active
    correction cycle early).
  - `enrollment_closed_at`: `occurred_at` of the latest `complete` or
    `withdraw` `enrollment_event` when that event is the enrollment's latest
    status event. An active or reinstated enrollment has **no** closure
    instant and is never eligible under `enrollment_closed_at`.
- **Refused targets:** Unfinalized drafts (0 versions), records with an open
  amendment, records whose class has no policy or a `retain` policy, records
  inside their retention period, records already disposed, and records covered
  by any active hold all return typed refusals.

### 3. Dependency closure vs. refusal without silent broadening

Consolebook **never** silently adds unselected records to a disposition batch:

- **Daily report referenced by a weekly summary (`summary_daily_link`):**
  - If any retained `weekly_summary` (draft, in review, open amendment, or
    finalized) references a version of a selected `daily_report` through
    `summary_daily_link`, and that `weekly_summary` is **not** also in the
    operator's confirmed selection, disposition **refuses** with a typed
    `DependentSummaryRetained` refusal naming the dependent summary record IDs.
  - When the operator explicitly selects both the `daily_report` and its
    eligible, unheld finalized `weekly_summary` in the same previewed and
    confirmed batch, execution removes `summary_daily_link` first and disposes
    both lineages in the same transaction.
  - Disposing a `weekly_summary` does **not** require disposing the daily
    reports it linked; only the summary's own `summary_daily_link` rows and
    summary lineage are removed.
  - Schedules that require destroying daily reports while retaining finalized
    weekly summaries that link them are explicitly **unsupported** under this
    v1 FK/lineage contract and fail closed rather than silently dropping links
    or broadening scope.
- **Released record-scoped holds (`record_hold.evaluation_record_id`):**
  - Migration `0015` defined `record_hold.evaluation_record_id REFERENCES
    evaluation_record(id)` with append-only triggers. Active holds block
    disposition; **released** holds must not block disposition of an otherwise
    eligible record.
  - The forward migration introducing disposition (`#212`/`#214`) replaces that
    blocking foreign key with an insert-time existence trigger (mirroring
    `audit_event`'s non-blocking subject reference in `0004`) so hold history
    stays append-only and never blocks lawful deletion of `evaluation_record`.
- **Ordinary immutability stays enforced:**
  - Existing `*_no_update` and `*_no_delete` triggers on `evaluation_version`,
    `acknowledgment`, `amendment`, `draft_snapshot`, `review_decision`,
    `contributor_event`, and frozen draft working-copy tables remain active
    for all ordinary paths.
  - Lawful disposition removes in-scope rows only through a transaction-scoped,
    schema-gated disposition context (verified against an in-transaction
    authorized `disposition_run` row, never a blanket `PRAGMA ignore_check_constraints`
    or disabled triggers). Raw ad-hoc `DELETE` statements outside an authorized
    disposition run continue to fail closed.

### 4. Exact-scope preview and confirmation invalidation

Disposition is a two-step preview-and-confirm workflow (`#211`, `#214`):

1. **Preview** evaluates the operator's explicit record-ID selection in a
   consistent read snapshot and returns the exact target records, every version
   number and content hash, effective policy version IDs and authorities,
   blocking holds or dependent summaries (if any), application-controlled copy
   inventory (local snapshots and configured off-server copy directory), an
   explicit statement of non-controlled copies (downloaded exports, external
   backups), and a deterministic SHA-256 **confirmation digest** plus a bounded
   expiry (15 minutes).
2. **Confirmation and execution** accepts the explicit record-ID set, the
   preview's `confirmation_digest`, a non-blank agency disposition authority
   reference, and a closed reason code. Inside `BEGIN IMMEDIATE` (`storage::write_tx`),
   the service re-verifies:
   - the caller still holds `execute_disposition`;
   - the confirmation digest has not expired and matches the freshly recomputed
     canonical preview digest byte-for-byte;
   - no policy revision, hold creation/replacement/release, amendment, summary
     link, or record state change occurred since preview.
   - Any drift returns a typed refusal (`StaleConfirmation`, `HoldBlocking`,
     `PolicyIneligible`, `DependentSummaryRetained`, `OpenAmendmentBlocking`,
     `CapabilityRequired`) and rolls back immediately (`storage::refuse`),
     changing nothing.

### 5. Controlled copy cleanup, residual bytes, and crash/retry states

A disposition is **not complete** merely because SQLite rows were deleted:

- **Atomic database phase (`db_committed_vacuum_pending`):**
  In one `BEGIN IMMEDIATE` transaction, execution creates the `disposition_run`
  row, records minimal `disposition_event` tombstones (and version-hash rows if
  policy permits), deletes all in-scope content tables (`evaluation_version`,
  `acknowledgment`, `amendment`, `draft_rating_modifier`, `draft_rating`,
  `draft_narrative`, `draft_snapshot`, `review_decision`, `contributor_event`,
  `evaluation_session`, `summary_daily_link`, record-linked `notice` rows, and
  `evaluation_record`), and appends a typed `audit_event`. A crash before
  commit rolls back completely with zero partial deletion.
- **Residual-byte reclamation (`copy_cleanup_pending`):**
  Immediately after commit (serialized against concurrent writers), the service
  runs `PRAGMA wal_checkpoint(TRUNCATE)`, `VACUUM`, and a second
  `PRAGMA wal_checkpoint(TRUNCATE)` so deleted `canonical_bytes`, narratives,
  and snapshots do not remain in SQLite freelist pages or `-wal` frames.
- **Safe backup replacement and copy cleanup:**
  1. Write the durable recovery ledger update first (`data/instance/disposition-ledger.json`,
     fsynced atomically via temp file + rename + directory fsync).
  2. Take a fresh validated post-disposition snapshot (`consolebook-<stamp>.db`)
     and copy it to `CONSOLEBOOK_BACKUP_COPY_DIR` (if configured) **before**
     deleting any pre-disposition snapshot, so a crash mid-cleanup never leaves
     the installation with zero valid backups.
  3. Delete all pre-disposition application-controlled files in `data/backups/`
     (`consolebook-*.db`, `consolebook-premigration-*.db`,
     `consolebook-prerestore-*.db`, `consolebook-prerestore-*.damaged`, and
     stale `.partial` files) and any future staged artifacts under
     `data/exports/` belonging to disposed records, plus all pre-disposition
     snapshots in `CONSOLEBOOK_BACKUP_COPY_DIR` (mirroring the updated recovery
     ledger there).
- **Explicit incomplete state and idempotent retry:**
  If `VACUUM`, clean-snapshot creation, local snapshot deletion, or configured
  off-server copy cleanup fails (for example, an unmounted share or read-only
  mount), the `disposition_run` remains in `db_committed_vacuum_pending` or
  `copy_cleanup_pending` with structured per-step diagnostics. `doctor`,
  `/api/backups/status`, and the retention UI report the disposition as
  **incomplete** — never `completed` — until an authorized retry (or recovery
  pass) finishes every remaining step and transitions the run to `completed`.

### 6. Recovery evidence and resurrection prevention

To prevent restoring a pre-disposition backup from silently resurrecting
destroyed records (`#213`, `#221`):

- **Durable recovery ledger:** Every completed database-phase disposition
  appends a minimal, non-sensitive entry to an installation-scoped ledger file
  under `data/instance/` (mirrored to `CONSOLEBOOK_BACKUP_COPY_DIR` when
  configured): `installation_id`, `disposition_run_id`, disposed
  `evaluation_record_id` values, disposed `content_hash` values, `disposed_at`,
  and `policy_authority` — **zero** record content, narratives, or personal
  names.
- **Restore enforcement (`restore` and `restore --verify`):**
  `consolebook restore` inspects the installation's recovery ledger (and any
  configured copy-directory mirror or operator-supplied `--disposition-ledger`
  path) before promoting a snapshot:
  - If the snapshot contains any `evaluation_record.id` or
    `evaluation_version.content_hash` present in the ledger, `restore`
    **refuses** plain promotion so disposed records cannot silently return.
  - Disposition-aware recovery either requires a post-disposition snapshot or
    runs an explicit, audited re-disposition pass on the staged scratch
    database (removing the ledger-listed records, vacuuming, recording the
    reconciliation event) before swapping the database into place.
  - `restore` reports clearly which ledger source(s) were consulted — and
    warns explicitly when restoring onto a clean server with no ledger
    available, never claiming resurrection protection it could not check.

### 7. Independent tombstones, metadata expiry, and honest verification

- **Minimal tombstone (`disposition_event`):** Retains only opaque IDs, record
  class, version count, earliest/latest finalization timestamps, optional
  version content/chain hashes (for chain continuity while permitted), policy
  version ID, agency authority reference, actor ID, UTC timestamp, method,
  outcome, and closed reason code. It never copies destroyed narratives,
  ratings, acknowledgments, amendment reasons, or personal names.
- **Independent metadata retention (`#218`):** Tombstones are governed solely
  by the `disposition_event` policy class (`retention_trigger = 'disposed_at'`,
  already modeled in migration `0015`). When that policy has `action = 'destroy'`
  and its elapsed days have passed (with no active installation hold), an
  explicit `execute_disposition` confirmation removes expired tombstone rows
  and prunes matching ledger entries once no pre-disposition application
  snapshots remain, leaving only a non-personal policy-boundary closure record.
- **Honest portable verification (`#215`–`#217`):**
  - While tombstones retain version hashes, record exports, trainee packets,
    installation exports, and `export verify` report disposed versions as
    **lawfully disposed and unavailable for content verification** — never
    "verified".
  - After tombstone expiration closes the chain, exports and `export verify`
    report an **intentional policy boundary** — never silent omission or a
    falsely clean recheck.

### 8. Explicit non-guarantees (outside Consolebook's control)

Consolebook refuses to overstate what software on one host can guarantee:

- **Downloaded exports and trainee packets:** Files already downloaded to
  browsers, workstations, email, or third-party archives are outside
  Consolebook's control.
- **External storage and host snapshots:** SAN/NAS snapshots, ZFS/btrfs CoW
  snapshots, VM disk backups, SSD wear-leveling blocks, and unmanaged copies of
  `consolebook.db` are outside Consolebook's control. `VACUUM` removes residual
  bytes from the live SQLite file and WAL only.
- **Pre-migration unlinked notices:** Existing `notice` rows created before the
  disposition migration carry no `evaluation_record_id` column (only generic
  workflow text with a trainee display name, zero narratives or ratings); they
  are documented honestly rather than heuristically deleted by substring match.
- **No supplied schedules or legal authority:** Consolebook never supplies
  default retention periods, jurisdictional schedules, or legal determinations.

## Consequences

- `#211`–`#219` implement this contract sequentially across authority/preview,
  tombstone/evidence schema, backup/restore ledger enforcement, confirmed
  execution + copy cleanup, record/packet/installation export reconciliation,
  tombstone expiration, and operator UI.
- `#220` (count-and-age backup retention) and `#221` (`restore --verify`) build
  directly on `#213`'s recovery ledger contract.
- Ordinary record immutability (PRINCIPLES.md 3, ADR 0011) remains unbroken:
  only confirmed, hold-free, policy-eligible disposition runs under
  `execute_disposition` may remove retained content.
