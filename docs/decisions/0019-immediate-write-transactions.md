# ADR 0019: Reserve the writer before transactional validation

- **Status:** Accepted
- **Date:** 2026-09-05
- **Issue:** [#27](https://github.com/FieldmouseWorks/consolebook/issues/27)
- **Amends:** [ADR 0003](0003-sqlite-connection-invariants.md)

## Context

The draft services introduced `storage::write_tx` in #29 and awaited refusal
rollback in #31. Earlier services still opened deferred transactions. Under
WAL, their validation reads could establish a snapshot that another writer
made stale before the write. SQLite then refuses promotion with
`SQLITE_BUSY_SNAPSHOT`; the service reports an internal error instead of its
existing domain conflict. A deferred reader can also fail promotion while
another writer still owns the reservation.

SQLite documents this behavior in [Isolation in SQLite](https://www.sqlite.org/isolation.html)
and its [transaction rules](https://www.sqlite.org/lang_transaction.html).
The issue's approved direction is to complete the immediate-transaction
retrofit, retaining validation semantics and refusal vocabulary.

## Decision

Every application-owned multi-statement write transaction starts through
`storage::write_tx`, whose sole implementation is `BEGIN IMMEDIATE`.
Reservation precedes the transactional validation reads; writes, notices,
and audit events commit together as before. A competing writer waits for
reservation, then evaluates those checks against committed state.

This covers program creation, version creation/replacement/publication/discard,
program imports, enrollments, enrollment and phase events, assignments,
training-session creation/update/close, session membership, user creation,
password-reset issuance/consumption, and setup issuance/initialization, as
well as the draft and record services that already use the helper.
Single-statement writes remain atomic SQLite statements; read-only snapshots
in program content, enrollment detail, draft workspace, and trainee packets
remain deferred and do not reserve the writer. Migration and backup mechanics
remain governed by their existing owners.

A typed refusal reached after reservation ends through `storage::refuse`,
which awaits rollback before returning it. Operations whose existing return
type is an outcome enum or `Option` await rollback directly. Unexpected
errors and cancellation retain SQLx's transaction-drop cleanup; they are not
translated into a fabricated domain refusal.

User creation retains its early username lookup to avoid unnecessary password
hashing, but repeats the case-insensitive uniqueness check inside the write
transaction. Setup-code issuance checks initialization and stores its code
under one reservation, so it cannot leave a setup code behind a successful
initialization. Password hashing remains outside write transactions.

## Boundaries and costs

The five-second connection busy timeout remains the bound on waiting for a
SQLite writer. A lock held beyond that limit, I/O failure, or pool exhaustion
can still produce an operational error. This change prevents stale-snapshot
promotion failures; it does not promise successful service under unlimited
contention, add retries, or change HTTP error vocabulary.

Existing capability and scope gates retain their placement. An immediate
transaction does not move a preceding authorization decision into its
snapshot; the qualification in the domain model still applies. This retrofit
does not establish a new authorization contract.

The reservation lasts through validation, including refusal paths, so those
reads briefly serialize with writers. Pure input validation and expensive
hashing stay outside it. Read-only exports and workspace loads keep their
existing snapshots. No schema, migration checksum, public service signature,
HTTP payload, configuration serialization, or canonical record format changes.

## Program ownership

To retrofit the large programs owner under CONTRIBUTING.md and #58:

- `programs/content.rs` owns configuration vocabulary and structural validation;
- `programs/persistence.rs` owns content loading/replacement helpers and
  transaction-owned program/version insertion; and
- `programs.rs` retains policy, typed refusals, summaries, and transaction
  orchestration, with compatible type/function re-exports.

This is #58's programs slice. Draft-workspace decomposition remains separate.

## Proof

`tests/write_transactions.rs` owns the contention harness and a direct WAL
stale-snapshot reproduction; its `programs`, `training`, and `accounts` child
modules own the service scenarios. A separate connection holds the writer
while competing operations are polled, then releases it. The scenarios assert
existing typed losers, or both legitimate successes (version numbering,
whole-draft replacement, session editing, and independent reset-code issuance).
An HTTP overlap race requires one 201 and one 409 with `interval_overlap`.
Cancellation racing with draft creation cannot commit both outcomes. Refusal
probes immediately reserve a separate connection with zero busy timeout.

Existing program/API, lifecycle/session, authentication, draft/review,
finalization, export, and browser suites provide regression coverage for the
retained service and serialization contracts.
