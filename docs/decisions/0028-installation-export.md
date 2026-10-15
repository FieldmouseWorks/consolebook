# ADR 0028: Complete installation export

- **Status:** Accepted
- **Date:** 2026-10-15

## Context

Milestone 5's exit is that "a center can leave with all of its data"
(`docs/roadmap.md`). Neither existing export carries all of it:

- The record export (ADR 0014) carries finalized versions only. It leaves out
  drafts, acknowledgments, amendments, signoff history, and everything that is
  not a record.
- The trainee packet (ADR 0015) carries one enrollment's history in a readable
  shape. It leaves out sessions and drafts that produced no finalized version,
  and everything outside an enrollment.

An inventory of the schema at migration 0016 found 47 tables:

- 23 feed some export;
- 5 are read only to authorize one;
- 19 are read by none. Those 19 include drafts and their review history,
  training sessions, retention policies and holds, notices, and the audit
  trail.

#98 asks for one export an agency could leave with, and use either to move to
another system or to answer a records request after leaving. It must carry the
same file-verifiable guarantees as the existing exports. The maintainer settled
four choices before any code was written, in #44's pattern (#98, "Decisions"):
one typed document per stored table; everything except credentials; a web
download; one pull request.

## Decision

### The archive is units plus one document per stored table

- Finalized versions travel as record-export units, byte for byte, written by
  the same code. Nothing is re-serialized, and the canonical bytes stay the
  record's authority (ADR 0011).
- Every other carried table is one canonical-JSON document,
  `tables/{table}.json`, shaped like the table as stored:
  - one object per row;
  - the stored column names, with typed values;
  - ids kept as references;
  - rows strictly ascending by key;
  - flags as booleans;
  - stored text unchanged.

  `evaluation_version` has its own document, without `canonical_bytes`, which
  travel as the units. The document and the units must agree row for row.
- A shape that mirrors the tables was chosen over domain-shaped documents like
  the packet's because completeness can then be enforced mechanically. There
  is one document per table, and one rule per column type, reference, and
  vocabulary. A reshaped format needs a hand-written mapping for every concept,
  and only review can prove that mapping complete. The packet remains the
  readable per-trainee view. This export is the complete one.
- Configuration travels in its tables, not as embedded program-version
  documents, because enrollments, records, and sessions reference
  configuration rows by id. The program-version export (ADR 0007) remains the
  portable, importable form of one program version.

### Everything except credentials, and the archive says so

- **Carried (43 tables):** finalized records and their metadata,
  acknowledgments, amendments, signoffs, enrollments and their lifecycle and
  phase history, assignments, training sessions and their trainers, drafts
  with their ratings, narratives, snapshots, contributor events, and review
  decisions, program configuration, people and capability grants, retention
  authority, policies, holds and releases, notices, and the audit trail.
- **Excluded:** login sessions and their `Secure` markers, password reset
  codes, setup codes, and `user.password_hash`, with reason `credential`. They
  mean nothing outside this installation and would endanger it in anyone
  else's hands. Signing in, issuing codes, and using them remain visible as
  audit events.
- The manifest carries a **ledger** naming every carried table, with its row
  count and hash, and every exclusion, with its reason. For a format version,
  the ledger is a closed set the verifier checks. "Nothing silently omitted"
  is therefore a property of the artifact, not a promise in a document.

### Completeness is enforced against the schema

- The producer's table specification is the one owning definition. For each
  table it records the key, each column's type, nullability, reference,
  agreement, and vocabulary, and each exclusion. Production and verification
  both read it.
- A test migrates a fresh database and compares every table and column in it
  with the specification. A migration that adds a table or column fails the
  test until this format decides how to carry it. A second test keeps the
  normative column tables in `docs/formats/installation-export.md` identical
  to the specification. These are rungs 1 and 2 of the correction ladder: one
  owning API, plus a test with a negative control.
- Closed vocabularies come from their owning modules: the stored `CHECK` lists,
  and `audit.rs`, `notices.rs`, and `capabilities.rs` for the four that have
  no `CHECK`. Values are parsed on production and on verification. A stored
  value the build does not know is refused at production, never passed
  through as text, so vocabularies only ever grow.

### Verification keeps ADR 0014's honest meaning

- `export verify` dispatches on the manifest's `format`. It applies:
  - the record export's container and unit checks, unchanged;
  - the ledger;
  - every document's hash, canonical form, members, types, nullability,
    vocabularies, key order, and count;
  - every stored foreign key as a reference that must resolve inside the
    archive;
  - the composite-key agreements that pin configuration to a program version;
  - the units' agreement with `evaluation_version`, and a carried predecessor
    for every unit.
- `audit_event.subject_*` is deliberately not resolved. It has no foreign key
  (migration 0004), so the append-only trail never blocks disposition of its
  subjects.
- The verifier does not re-run every stored constraint, such as text lengths
  or speaker rules. The format says so. A verified archive is consistent with
  its stated fingerprints and its format: not tamper-proof against its
  producer, and not proof of origin.

### One snapshot, streamed like the record export

- The unit list, every document, and every count are read inside one read
  transaction, so the archive describes one committed state. Unit bytes are
  then streamed and checked against the stored hashes the manifest states, as
  ADR 0014 streams them. No database transaction is held across the transfer.
- Documents are held in memory until they are written. For a pilot-sized
  installation that is megabytes. Streaming them is a later change if
  measurements call for it, not a format change.

### Who may produce it, and how

- Only `export_records` holders may produce it. That capability already
  exports every finalized record installation-wide (ADR 0014). No narrower read
  rule could cover people, drafts, and the audit trail.
- It is offered on the home page beside the installation record export, at
  `GET /api/exports/installation`. Each export is audited as
  `installation_exported`, with the actor and no content.
- There is no offline CLI export in this decision. An agency leaves through
  the running installation. The archive is unencrypted, like backup copies
  (ADR 0027), and the format says so.

## Consequences

- A schema change and this format now move together. Any migration that adds
  or changes a table or column bumps `format_version` and updates the format
  document, producer, verifier, and fixtures (CONTRIBUTING.md). The coverage
  test fails a table or column the format has not decided on. Vocabularies
  grow within a format version and never lose a value; a reader meeting an
  unknown value reports it, as this verifier does.
- A recorded fingerprint pins the structure the specification holds per
  `format_version`: tables, keys, columns, text forms, references,
  agreements, which columns are vocabularies, exclusions, and columns carried
  as units.
- A golden version-1 archive pins the manifest shape, and the verifier's
  rules as far as its data exercises them. Skipping a version bump for a
  change that makes that archive read differently therefore fails.
- A loosened rule that has a negative control fails that control.
- Review remains the guard for three things: a tightening the golden data
  happens to satisfy, a rule loosened with no control, and a recorded
  fingerprint edited together with the specification.
- A build refuses an unknown format version by name. The ADR for a later version decides how earlier
  archives keep verifying.
- The format exposes the storage schema's names and shapes to whoever reads an
  export. That is the point (the agency's data as it is held), and the format
  document is now the public description of that schema.
- Disposition (#64) will remove units and add tombstones. Its decision must say
  how this export carries tombstones and reports a predecessor that was
  lawfully disposed, since "every unit's predecessor is carried" will no longer
  hold for disposed versions.
- The record export's `installation` scope remains as the records-only
  archive. Nothing about it changes.

## Alternatives considered

- **Domain-shaped documents, as in the trainee packet.** More readable, but
  completeness would rest on review, and each concept needs its own spec and
  code. Not chosen (maintainer, #98).
- **One trainee packet per enrollment plus extras.** Reuses packet code, but
  sessions, drafts, people, retention, and audit still need new documents, and
  the verifier would have to nest formats. Not chosen.
- **A database snapshot with a manifest.** Complete by construction, but not
  file-verifiable in the sense ADR 0014 established, and unreadable without
  SQLite and the schema. It also carries credentials unless they are scrubbed,
  which makes it not a snapshot any more. Backups already cover the
  restorable-copy need (ADR 0006).
