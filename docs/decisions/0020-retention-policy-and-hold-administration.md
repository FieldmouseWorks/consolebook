# ADR 0020: Retention policy and hold administration

- **Status:** Accepted
- **Date:** 2026-09-05
- **Issues:** [#65](https://github.com/FieldmouseWorks/consolebook/issues/65),
  [#64](https://github.com/FieldmouseWorks/consolebook/issues/64)
- **Implements part of:** [Milestone 5 decisions](https://github.com/FieldmouseWorks/consolebook/issues/44)

## Context and delivery boundary

The approved retention design requires versioned policy, typed holds,
explicit authority, exact-scope confirmation, destruction, and independently
retained disposition evidence. These are separate contracts. Existing data
also lives in working copies, review snapshots, acknowledgments, amendments,
linked summaries, exported archives, and backups. A deletion implementation
must account for those copies and restore behavior before claiming completion.

This first vertical stage delivers policy and hold administration. It adds no
record deletion exception, disposition execution, eligibility verdict, or
export-format change. #64 remains open for execution, copy/recovery scope,
partial failure and retry, tombstones, and policy-boundary verification.

## Decision

### Explicit administration authority

`manage_retention` is a new capability for policy/hold administration and
reading its installation-wide metadata. No existing or future role bundle
includes it. A `manage_users` holder explicitly grants or revokes it for an
existing user with a reason. This uses the existing authority for managing
users; it does not give administrators implicit policy or hold access.
The grant/revoke and its attributed authority event and audit event commit
in one immediate transaction. Duplicate state changes refuse typed.

Destruction will require a separate explicit capability in #64. This
administration capability alone cannot authorize destruction.

Every retention service checks the required capability on the same connection
and transaction as the governed reads/writes. A writer waiting for reservation
checks the committed grant state after it acquires the reservation. Read-only
snapshots remain deferred. Typed refusals await rollback (ADR 0019).

### Versioned policy

Each immutable policy revision names one of four classes: daily reports,
weekly summaries, phase evaluations, or disposition events. There is one
ordered policy history per class. A new revision takes the id of the current
version it supersedes; stale replacement is refused, including concurrent
attempts against the same version. Missing policy never authorizes destruction.

A policy carries an agency-supplied authority reference, a typed trigger,
minimum retention days, scheduled action, reason, actor id, and UTC timestamp.
Evaluation triggers are finalization or enrollment closure; disposition-event
policy uses disposition time. These are explicit elapsed days of 24 hours,
not calendar months or years. Periods range from 0 to 365250 days. The `retain`
action has zero days and authorizes no destruction; `destroy` records the
schedule intent, subject to the later workflow's complete checks.

No jurisdictional schedule, recommended duration, default destruction policy,
or assumed legal authority ships with the application. A policy is stored
configuration, not a claim that the application can execute that schedule yet.
Disposition-event policy is independently versioned in preparation for #64;
there are no disposition events to prune in this stage.

### Holds

A hold has exactly one scope: installation, enrollment, or evaluation record.
Installation scope covers every record; enrollment scope covers records of
that enrollment; record scope covers only that record, including its lineage.
Scope is established by typed identifiers and relationships, never names,
authority text, date heuristics, or keyword matching. Scope selectors display
human-readable labels but submit the identifiers. Records may be held before
finalization.

The closed hold-kind set is litigation, anticipated litigation, audit,
investigation, public-records request, and other authority. Every kind carries
an authority reference and reason; `other` uses those fields to name the
agency's configured authority. There is no automatic expiry.

Holds and releases are append-only. A release records its actor, timestamp,
and reason; it never changes or deletes the original hold. Replacing a hold
creates an attributed successor, and an insert trigger releases its active
predecessor with matching attribution in the same transaction. A failed insert,
audit failure, or competing release cannot leave the predecessor silently
released. Replacement/release of an already released hold is refused.

The lookup returns applicable active holds for one existing record. An empty
list is only a hold result. The UI never presents it as permission to destroy.

### Storage and ownership

Migration 0015 adds policy, hold, release, and authority-event tables and their
constraints and append-only guards. Existing record immutability and historical
migration checksums are unchanged. Service validation adds bounded, nonblank
authority/reason checks; schema checks enforce vocabulary, field combinations,
policy succession, valid scope references, and release/replacement pairing.
Audit rows carry only typed event and subject identifiers, not these reasons.

`retention/types.rs` owns vocabulary, `policies.rs` owns policy revisions,
`holds.rs` owns hold lifecycle and scope resolution, and `authority.rs` owns
explicit grants. The parent owns shared transaction/authorization boundaries;
`retention_http.rs` only adapts HTTP. The operator page has separate policy,
hold-editor, and authority components. Shared request/error transport moves to
`web/src/lib/api/transport.ts` with compatible legacy API imports; the new
retention client has its own module. Remaining web decomposition stays in #59.

## Proof and remaining work

Integration tests cover explicit grants and revocation while a write waits,
policy succession and stale conflicts, all hold kinds and scope matching,
replacement/release races, injected failures with rollback, append-only/raw
shape restrictions, and typed unauthorized HTTP responses. A browser scenario
uses the operator controls for explicit delegation, policy revision, hold
replacement/release, and revoked access. Existing repository gates cover the
unchanged record/export behavior and API transport imports.

Policy interpretation at execution, exact-scope preview and confirmation,
record-copy removal, backup/restore interaction, independent tombstone expiry,
and portable policy-boundary evidence remain #64 work. Administration history
is retained in this stage; no claim of a complete retention implementation or
lawful-disposition workflow follows from these tables or tests alone.
