# ADR 0021: Trainee access to their own complete signoff history

- **Status:** Accepted
- **Date:** 2026-09-12

## Context

`task_signoffs::matrix` (ADR 0013) is the recording interface's read: it
projects the tasks of the enrollment's **currently pinned** program
version and answers each one's current state from its latest signoff row,
with a count of the rest. It is gated by `lifecycle::may_read` — the
training-history rule of `assign_training`, or `view_assigned_records`
with an active assignment — so a trainee holding `view_own_records`
cannot read the signoffs recorded about them at all (issue #49).

That gate is correct for what it protects: the matrix drives recording
controls, and recording a signoff is not something a trainee does. But it
leaves the interface disagreeing with the artifact that already exists.
The trainee packet (ADR 0015) carries the **full** signoff history to the
trainee, on the recorded principle that a record about them which other
readers can see but they cannot would make the packet incomplete by
design. Meanwhile the enrollment's version-change semantics (ADR 0008)
mean the matrix alone could never serve them: signoffs recorded under an
earlier pin point at an earlier version's tasks, so a projection through
the current pin makes them invisible rather than merely unavailable.

An owner decision was needed on the durable read contract: what a trainee
may read, and whether that read is the matrix with a wider gate or a
contract of its own.

## Decision

### A separate complete-history read, not a wider matrix gate

`task_signoffs::history` is a new typed, service-owned read:

- it is driven by the append-only `task_signoff` rows themselves, so
  every retained row is reported — first signoffs and overrides alike —
  in recorded order **within its task** (`signoff_id` ascending, the
  packet's own recorded order), with each task grouped once so a reader
  never has to reassemble a task's rows;
- it reports every task with any signoff, plus every task of the
  currently pinned version whether or not it was signed off, so "not
  signed off" stays distinguishable from "revoked";
- each task carries the program version it belongs to (id, number, and
  stored label) and whether that is the enrollment's current pin, so a
  prior version's signoffs stay discoverable **and labelled** after a
  version change rather than dropped, reattributed, or silently presented
  as current;
- each row carries the closed kind, the recorded reason, the signer's
  identity and **stored name snapshot**, and the recorded instant.

A task's current state is the latest retained row **for that task row**,
which is what ADR 0013's per-pinned-version rule already implies. The
history therefore has one stated limit: if an enrollment returns to a
version it pinned before, the rows recorded under that earlier epoch
answer for the re-pinned version again, while the act recorded under the
intervening version stays reported as that version's vocabulary. The read
does not reconstruct epochs, because reading the enrollment's event stream
to decide which rows "count" would make the answer depend on a second
table rather than on the rows it reports; a reader that needs epoch
history reads the event stream (the packet's enrollment document does).

`matrix` is unchanged, including its `lifecycle::may_read` gate: nothing
about the recording interface's authorization moves.

### The authorization line

`history` admits the readers `lifecycle::may_read` already admits, plus
**the trainee themselves on their own enrollment** when they hold
`view_own_records`. It does not widen `lifecycle::may_read`, so no other
training-history read or editing workflow moves with it, and it grants no
write: recording, overriding, and revoking keep exactly the authority
`record` already required (ADR 0013), so a trainee-only account gains no
signoff creation, revocation, contest, review, or administrative
capability. An account holding separate authority keeps it — no rule
guesses from a role label (ADR 0010).

The rule is evaluated on the read's own connection, inside its
transaction, so permission and contents describe one committed state, and
the request never acquires a second pooled connection while holding one.
Authorization timing is therefore "at the read", like every other read
contract; it is not continuous revocation of data already delivered.

### Shape, ownership, and presentation

- The response is a plain typed contract (`SignoffHistory` and its rows),
  reused vocabulary from ADR 0013's closed sets, and deliberately no
  second interpretation of the table: `tests/signoff_history.rs` compares
  the read against the packet's own signoff document row for row on
  identity, kind, reason, signer, and instant for the same enrollment.
- `web/src/lib/api/signoffs.ts` owns the client contract, following
  `web/src/lib/api/retention.ts`; `web/src/lib/signoffs/` owns the
  presentation.
- The presentation is **read-only** and lives on `/records`, where the
  trainee already finds their records and packets: each enrollment's task
  states with their latest signoff, the full retained chain behind an
  accessible expandable list, and earlier program versions in their own
  labelled section. There are no recording, contesting, or acknowledgment
  controls, and no new authorization authority in the UI.
- No persisted copy, no schema change, and no automatic rewriting of
  historical signoffs: the history is read from the rows the system
  already retains.

## Consequences

### Positive

- The interface agrees with the packet on the same data, by test rather
  than by intention, so a trainee is not required to download and unzip an
  archive to see what was signed off about them;
- version changes stop hiding retained acts: the history reports the
  vocabulary each signoff was recorded under, which is what ADR 0013's
  "signoff state is per pinned version" actually implies;
- the recording gate and the read gate are two named rules with one owner
  each, instead of one gate quietly widened to serve two purposes.

### Costs

- Two reads now describe `task_signoff`: the pinned-version matrix for
  recording, and the complete history for reading. Both are small, both
  read the same stored columns, and the agreement test is the price of not
  merging them into one contract that would have to serve two callers with
  different scopes;
- the history's `in_current_version` flag and per-task version label are
  presentation of the pin, and a task's current state comes from its own
  latest retained row, so a reader that wants epoch history must still
  consult the enrollment's event stream (the packet's enrollment document
  already does): re-pinning a version earlier in the enrollment's life
  makes that epoch's rows answer for the pin again;
- a trainee-only reader now holds a view of their own training history
  they did not have, which is the intended correction rather than a leak:
  the packet already carried it.

## Rejected alternatives

- **Widening `lifecycle::may_read` to admit `view_own_records`:** it is
  named for one rule and used by reads whose scope is not "my own
  record"; a trainee's own view would silently join enrollment history,
  phase history, and assignment-scoped reads that were never reviewed for
  it. The trainee's access is its own contract because its scope is its
  own.
- **Making `matrix` return the full history:** the matrix's shape is the
  pinned vocabulary with recording controls; returning prior-version tasks
  in it would present tasks the pinned version no longer defines beside
  controls that cannot act on them, and would put the history behind the
  recording gate again.
- **A client-side merged view from `matrix` plus a packet download:** the
  packet is an archive, and requiring a trainee to download and unzip it
  to read their own signoffs is the interface gap this ADR closes.
- **Persisting a per-task "current signoff" row beside the history:**
  ADR 0013 already rejected mutable signoff state; the latest retained row
  answers the current state without a second writable authority.
