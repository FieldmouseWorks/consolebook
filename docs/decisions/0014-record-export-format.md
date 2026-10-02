# ADR 0014: Record export format and verification

- **Status:** Accepted
- **Date:** 2026-09-01

## Context

Milestone 5 makes an installation leave-able: "a center can leave with
all of its data and can prove recovery from a clean installation"
(`docs/roadmap.md`). Milestone 4 finished the record substrate —
canonical bytes with content and chain hashes (ADR 0011), successor
versions (ADR 0012), record schema 2 (ADR 0013) — but a finalized
version is readable only through the installation that holds it.
`docs/records-integrity.md` requires export round-trip tests and
honest verification wording, and #44 decision 1 settled the owner
choice: the stored canonical bytes travel verbatim beside a manifest,
multi-record exports are an archive of the same units plus an archive
manifest, and verification needs nothing but the export. This ADR
fixes the format (`docs/formats/record-export.md`), its scopes, who
may export, and what verification claims (#45; Milestone 5 slice 1).

## Decision

### Units carry stored bytes, never a re-serialization

- A unit is one finalized version's `canonical_bytes` copied from
  storage, beside a unit manifest naming the installation, the record
  and version identity, the stored `record_schema`, both stored hashes,
  the predecessor's content hash, the export instant, and the format
  version. Schema-1 and schema-2 bytes travel as stored; nothing
  upgrades, rewraps, or reformats a record on the way out.
- Manifests are themselves canonical JSON under the record format's
  JCS subset, so an export is deterministic: the same scope exported
  at the same instant is byte-identical.

### Archives are ZIP containers, one format for every scope

- Every export, a single version included, is a ZIP archive with
  stored entries, a fixed entry order, fixed entry metadata, and an
  archive manifest listing every unit with its identity and hashes. One
  container means one reader, one verifier, and one document for all
  four scopes: version, record (every retained version), enrollment
  (every finalized version of its records), and installation.
- The archive holds nothing this format does not name; extra entries
  are verification findings, so a tampered or repacked archive cannot
  hide content beside the records.
- A scope with no finalized version is a typed refusal
  (`nothing_to_export`), never an empty archive presented as a
  complete export.

### Verification from the export alone

- The verifier recomputes the content hash over the bytes, proves the
  bytes are canonical by re-serialization, reads them as a typed
  envelope of their declared record schema (`record_envelope`, the
  reading side of ADR 0011's shape: every named member, typed, and no
  other), cross-checks the envelope's own `record` and `instance`
  members against the manifests,
  recomputes the chain hash from the carried predecessor hash, links
  predecessors present in the same archive, checks the listed units
  against the declared scope as far as the archive allows, and walks
  the container's central directory itself so an entry name written
  twice is a finding rather than whichever copy one reader happens to
  pick. It reports per unit and per archive with typed findings; the
  verdict is `verified` only when every check passes.
- It ships as a library function and as
  `consolebook-server export verify <archive>`, which opens no data
  directory — the artifact is checked wherever it landed.
- Honest limits (ADR 0010, ADR 0011): a verified export is internally
  consistent with its stated fingerprints. It does not prove which
  installation produced it, nor completeness against what that
  installation held; the scope member is the exporter's statement of
  intent. Signatures (the future signed mode) attach beside the hashes
  without changing a record byte.

### Export is a read in a portable shape

- Authorization reuses the read contracts that exist rather than
  adding parallel ones: a version or record exports for whoever may
  read the record (workflow readers, and the trainee on their own
  finalized record — every retained version, per ADR 0012); an
  enrollment exports for whoever may read its training history; the
  whole installation exports for `export_records` holders, the
  explicit administrative authority PRINCIPLES.md 10 demands for
  breadth. A unit contains exactly what its reader can already read
  through the API; the capability gates breadth, not format.
- Every export is audited (`record_exported`) with actor and subject
  and never with content, in the append-only audit trail
  (`docs/records-integrity.md`).

### Nothing is retained on disk

- Exports are produced on request and delivered as downloads; this
  slice writes nothing under `data/exports/`. A future persisted export
  (scheduled exports, large archives) must register each file so
  lawful disposition can reach derived exports in scope
  (`docs/records-integrity.md` step 4); until then there are none.

## Consequences

### Positive

- a record leaves the installation exactly as it was sealed, and any
  reader of the format document can check it without Consolebook;
- one container and one verifier cover single versions, records,
  enrollments, and the whole installation, and the trainee packet
  (slice 2) can be built from the same units;
- determinism makes exports comparable and testable byte for byte,
  the property `docs/records-integrity.md` asks of every record
  representation; and
- verification wording carries the same honesty as in-database
  verification — consistency, not tamper-proofing.

### Costs

- a new dependency (`zip`, without default features: stored entries
  only) enters the one executable;
- the format version is shared by both manifests, so any manifest
  change bumps it for every export;
- an archive pins the predecessor by content hash only, so an
  exported successor without its predecessor verifies its own chain
  hash but cannot prove the predecessor's bytes — reported as *not in
  export*, never inferred; and
- on-demand exports mean nothing on disk to dispose of and nothing to
  resume: an export is one response, and it is now streamed — written to
  the response as it is produced (#47). What still scales with an
  installation is its unit count, not its bytes: the unit metadata the
  archive manifest lists, the serialized manifest, and the container's
  central directory are each O(units), and the database driver buffers a
  bounded number of rows per query. The corpus of stored payloads is
  never held, one unit's bytes are held at a time, and the browser's own
  download helper buffers the response it saves, separately;
- the delivery and its failure share one bounded channel between the
  producer and the response. A client that stops reading stops the
  producer instead of growing a queue, and a client that stops reading
  for longer than the stall bound loses the transfer. Whether a transfer
  was complete is an explicit fact the producer records only when the
  archive was produced and its tail flushed — not the channel closing,
  which a client that outlasted the failure signal would otherwise read
  as success. Anything else ends the body in an error: an incomplete
  download the verifier refuses, never a complete export;
- the export holds one pooled connection at a time. Its audit event is
  written in its own short write transaction, committed before the read
  snapshot the manifest and payloads share, and that snapshot is a reader
  only (ADR 0019). Exports sharing a connection pool therefore never hold
  one connection while waiting for another, and an export never reserves
  the writer for the length of a download;
- the audit event records the export the installation produced from the
  state at its recorded instant, not the operator's receipt: a delivery
  that fails part way leaves the record, and a scope that holds nothing to
  export is refused before any record is written.

## Rejected alternatives

- **JSON re-serialization of the envelope (a "pretty" export):** the
  hash is over the specified bytes; any re-serialization is either a
  no-op (pointless) or a change (a record that no longer verifies).
- **A single JSON document embedding the bytes as strings or
  base64:** embedding re-encodes the bytes, and base64 hides the
  record from a human reader; the decision was units *beside* a
  manifest, unchanged.
- **Tar or a bespoke container:** ZIP opens on every operator
  platform without tooling; a bespoke container would need its own
  reader in every verifier.
- **A separate `export_records` gate on every scope:** a trainee may
  already read every retained version of their own record and a
  reviewer every record in their scope; forcing an administrator to
  produce those exports adds a gatekeeper without adding protection,
  which is not what PRINCIPLES.md 10's explicit-authority rule is for.
- **Exporter identity in the manifest:** provenance of a file is the
  audit trail's job; putting an operator's identity in every artifact
  that leaves the system is personal data the record does not need.
