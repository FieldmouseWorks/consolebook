# Record Export Format

Portable, verifiable exports of finalized evaluation versions (ADR 0014;
`docs/records-integrity.md`; #44 decision 1).
`consolebook-server/src/record_export.rs` produces exports and
`consolebook-server/src/export_verify.rs` verifies them;
`tests/record_export.rs` proves the round trip, determinism, and every
verification finding. This document is normative: the implementation
follows it, not the other way around.

An export never re-serializes a record. The stored canonical bytes of
each version (ADR 0011, ADR 0013) travel byte for byte, beside a
manifest that names them. Verifying an export needs nothing but the
export.

## Vocabulary

- **Unit** — one finalized version as exported: its canonical record
  bytes and its unit manifest.
- **Archive** — the container holding one archive manifest and one or
  more units. Every export, a single version included, is an archive,
  so one reader and one verifier cover every scope.
- **Scope** — what the archive claims to contain: one version, one
  record (every retained version, superseded originals included), one
  enrollment (every finalized version of its records), or the whole
  installation.

## Container

The container is a ZIP archive (APPNOTE 6.3):

- every entry uses compression method 0 (stored); nothing is encrypted;
  ZIP64 structures appear only when the container's size or entry count
  requires them (a record is kilobytes, so no single entry ever does);
- entries are files only — no directory entries — named with forward
  slashes and ASCII characters;
- entry order is the archive manifest first, then units in ascending
  (`record_id`, `version_number`) order, `record.json` before
  `manifest.json` within a unit, then `disposition/evidence.json` when
  disposition evidence or policy boundary checkpoints apply to the scope;
- every entry's modification time is the export instant (DOS time, UTC
  as written, so seconds round down to even), and every entry carries
  Unix permissions `0644`;
- the archive holds nothing but what this document names; anything else
  is a verification finding.

Entry names:

```text
manifest.json                                   archive manifest
records/{record_id}/v{version_number}/record.json    canonical record bytes
records/{record_id}/v{version_number}/manifest.json  unit manifest
disposition/evidence.json                       portable disposition evidence (optional, v2+)
```

`{record_id}` and `{version_number}` are the decimal integers of the
unit's identity with no padding. The archive manifest states each
unit's `path` as the directory prefix (`records/12/v2`) and the path
must equal the derived form.

## Determinism

An archive is a pure function of its scope's stored rows and the
export instant: the same scope exported at the same instant produces
identical bytes. Manifests are canonical JSON under the record format's
JCS subset (ADR 0011): UTF-8, members sorted by code point, no
insignificant whitespace, integers only. Record bytes are copied from
storage unchanged.

## `record.json`

The version's `canonical_bytes` exactly as stored — a schema-1 or
schema-2 envelope under `jcs-v1`, presented under its own stored
`record_schema`. The export never upgrades, rewraps, or reformats them.
The envelope's own `record` and `instance` members are what the
verifier cross-checks against the manifests: the bytes commit to their
identity and lineage on their own (ADR 0011).

## Unit manifest (`records/…/manifest.json`)

```json
{
  "chain_hash": "…64 lowercase hex…",
  "content_hash": "…64 lowercase hex…",
  "exported_at": 1756753200,
  "format": "consolebook-record-unit",
  "format_version": 1,
  "installation_id": "…",
  "predecessor_content_hash": null,
  "record_id": 12,
  "record_schema": 2,
  "version_number": 1
}
```

| Member | Type | Meaning |
| --- | --- | --- |
| `format` | string | Always `consolebook-record-unit` |
| `format_version` | integer | `1`; bumped by any change to the unit manifest's shape |
| `installation_id` | string | The exporting installation's identity (`instance.installation_id`) |
| `record_id` | integer | The record's instance-local identity (positive) |
| `version_number` | integer | This version's number within the record (from 1) |
| `record_schema` | integer | The stored envelope schema of `record.json` |
| `content_hash` | string | The stored SHA-256 of `record.json`, lowercase hex |
| `chain_hash` | string | The stored integrity-chain hash (ADR 0011), lowercase hex |
| `predecessor_content_hash` | string or `null` | The prior version's content hash; `null` exactly when `version_number` is 1 |
| `exported_at` | integer | The export instant, UTC unix seconds; identical across the archive |

A unit manifest repeats what the archive manifest says about the unit
so that a unit directory stands on its own; the two must agree.

## Archive manifest (`manifest.json`)

```json
{
  "disposition_evidence": {
    "path": "disposition/evidence.json",
    "sha256": "…64 lowercase hex…"
  },
  "exported_at": 1756753200,
  "format": "consolebook-record-export",
  "format_version": 2,
  "installation_id": "…",
  "scope": { "kind": "record", "record_id": 12 },
  "units": [
    {
      "chain_hash": "…",
      "content_hash": "…",
      "path": "records/12/v1",
      "predecessor_content_hash": null,
      "record_id": 12,
      "record_schema": 2,
      "version_number": 1
    },
    {
      "chain_hash": "…",
      "content_hash": "…",
      "path": "records/12/v2",
      "predecessor_content_hash": "…",
      "record_id": 12,
      "record_schema": 2,
      "version_number": 2
    }
  ]
}
```

| Member | Type | Meaning |
| --- | --- | --- |
| `format` | string | Always `consolebook-record-export` |
| `format_version` | integer | `2` (legacy `1` archives without `disposition_evidence` remain verified under explicit compatibility dispatch) |
| `installation_id` | string | As in the unit manifest |
| `exported_at` | integer | The export instant, UTC unix seconds |
| `scope` | object | What the archive claims to contain (below) |
| `units` | array | Every retained unit, ascending by (`record_id`, `version_number`), no duplicates. May be empty in version 2 **only** when `disposition_evidence` accounts for a fully disposed or policy-boundary-closed scope — never for an ordinary empty export |
| `disposition_evidence` | object or `null` | Version 2 only: `{ "path": "disposition/evidence.json", "sha256": "…" }` when any disposition tombstone or applicable policy boundary checkpoint exists for the scope, or `null` otherwise |

`units[]` members are `path`, `record_id`, `version_number`,
`record_schema`, `content_hash`, `chain_hash`, and
`predecessor_content_hash`, with the unit-manifest meanings.

### `disposition/evidence.json` (version 2)

When `disposition_evidence` is non-null, `disposition/evidence.json` is a
canonical [`consolebook-disposition-evidence` v1](disposition-evidence.md)
document matching `manifest.installation_id` and `manifest.exported_at`:

- `dispositions` carries minimal tombstones (and optional retained version
  hashes, never destroyed record content) for lawfully disposed records in the
  requested scope, strictly ascending by `evaluation_record_id`.
- `boundary_checkpoints` carries policy-required metadata closures
  (`disposition_boundary_checkpoint`) for `enrollment` and `installation`
  scopes, strictly ascending by `checkpoint_id`; single-record and
  single-version scopes carry no boundary checkpoints.
- No `evaluation_record_id` may appear both in `units` and in `dispositions`.
- An evidence document with both `dispositions` and `boundary_checkpoints`
  empty is invalid; the manifest sets `disposition_evidence` to `null` instead.

### Compatibility with format version 1

Format version 1 (`format_version: 1`) carries no `disposition_evidence`
member and requires `units` to be non-empty. `export_verify` explicitly
dispatches version 1 and version 2 by name so previously produced version-1
archives continue verifying without migration, and refuses any unknown version
(such as `3`) as `UnsupportedFormat`.

### `scope`

`kind` is one of:

- `version` — with `record_id` and `version_number`; either one retained unit
  matching that identity, or (in version 2) zero units plus a disposition
  entry for `record_id` with `1 <= version_number <= version_count`.
- `record` — with `record_id`; either every retained version of that record,
  or (in version 2) zero units plus the disposition entry for `record_id`.
- `enrollment` — with `enrollment_id`; every retained finalized version of
  that enrollment's records, plus (in version 2) every disposition tombstone
  for that enrollment and any installation policy boundary checkpoints.
- `installation` — no further members; every retained finalized version the
  installation holds, plus (in version 2) every disposition tombstone and
  policy boundary checkpoint.

The scope is the exporter's statement of intent. Verification checks
the archive against itself — a `version` scope must match its single unit or
disposed version, a `record` scope must hold only that record's versions or
tombstone, and an `enrollment` scope's disposed entries must name that
enrollment — but file-only verification cannot know whether an installation
held additional records the archive omits, nor can it prove legal completeness
or verify destroyed bytes.

## Verification

A verifier reads only the archive. It reports, per unit and for the
archive as a whole, and its verdict is `verified` only when every check
below passes. Wording stays honest (ADR 0010, ADR 0011,
`docs/records-integrity.md`): a verified export is internally
consistent with its stated fingerprints. Without the future signed
mode nothing in the archive proves which installation produced it, and
the checks are not tamper-proofing against whoever produced the file.

Archive checks:

1. the container is a readable ZIP archive whose central directory names
   each entry once — a name written twice is a finding, because
   extraction tools disagree on which copy they take;
2. `manifest.json` exists, parses, is canonical JSON, and carries a known
   `format` and `format_version` (`1` or `2`);
3. `units` is strictly ascending by (`record_id`, `version_number`) with no
   duplicate identity, and each `path` equals the derived form. In version 1
   `units` must be non-empty; in version 2 `units` may be empty **only** when
   `disposition_evidence` is present and valid for the scope;
4. when version 2 carries `disposition_evidence`, its `path` is
   `disposition/evidence.json`, its `sha256` matches the entry's bytes, the
   entry passes all [`consolebook-disposition-evidence` v1](disposition-evidence.md)
   checks (`authenticity: "self_consistency_only"`, allowlisted fields,
   continuous hash lineage when `hashes_retained` is true), its
   `installation_id` and `exported_at` agree with `manifest.json`, it is not
   empty, and no `evaluation_record_id` appears in both `units` and
   `dispositions`;
5. the listed units and disposition entries fit the declared scope as far as
   the archive can tell;
6. every entry in the container is `manifest.json`, one of a listed unit's
   two files, or (in version 2 when referenced) `disposition/evidence.json` —
   an unlisted entry is a finding; and
7. every listed file exists.

Verdict honesty (ADR 0030 §7):

- A verified retained-only archive reports verified units consistent with
  their stated fingerprints.
- A verified mixed archive reports retained units separately from lawfully
  disposed records (`disposed_hash_continuity_only` or
  `disposed_content_and_hashes_unavailable`) and intentional policy boundaries
  (`intentional_policy_boundary`).
- A verified fully disposed archive reports zero retained units alongside its
  disposition tombstones (`FullyDisposed`), never as an ordinary empty export
  and never claiming destroyed bytes were verified.
- A verified archive with zero retained units and zero tombstones reports its
  intentional policy boundaries alone (`PolicyBoundaryOnly`): boundary
  checkpoints are installation-wide, so it claims neither that the scope held
  disposed records nor that it was empty.

Unit checks, for every listed unit:

1. `manifest.json` parses, carries the known `format` and
   `format_version`, and agrees with the archive entry and the archive
   manifest on every shared member (`installation_id`, `exported_at`,
   identity, schema, hashes);
2. `content_hash`, `chain_hash`, and a non-null
   `predecessor_content_hash` are each 64 lowercase hex characters, and
   `content_hash` equals SHA-256 over the bytes of `record.json`;
3. `record.json` parses as JSON and re-serializing it under the
   canonical subset reproduces the identical bytes — the bytes are
   canonical, so the hash is over the specified representation;
4. `record.json` is an envelope of a known record schema: every member
   ADR 0011 (schema 1) or ADR 0013 (schema 2) names, with its type,
   every nullable member present, no member the schema does not name,
   `daily_reports` present exactly for schema 2, and `attachments`
   empty (no known schema carries attachments);
5. the envelope agrees with the manifest: `record.id`,
   `record.version_number`, `record.record_schema`,
   `record.predecessor_content_hash`, `instance`, and
   `canonicalization` (`jcs-v1`);
6. `chain_hash` equals
   `SHA-256("consolebook-version-v1" || 0x00 || predecessor || bytes)`
   with `predecessor` the raw 32 bytes of `predecessor_content_hash`,
   or 32 zero bytes when it is `null`;
7. `record_id` and `version_number` are positive integers, and
   `predecessor_content_hash` is `null` exactly when `version_number`
   is 1; and
8. when the archive also lists (`record_id`, `version_number - 1`),
   that unit's `content_hash` equals this unit's
   `predecessor_content_hash` — reported as *linked*; a predecessor the
   archive does not carry is reported as *not in export*, which is not
   a failure (a single-version scope is legitimate), and a first
   version reports *none*.

`consolebook-server export verify <archive>` runs exactly these checks,
prints one line per unit and every finding, and exits non-zero unless
the verdict is `verified`. It opens no data directory.

Verification deliberately does not check the container's entry order,
modification times, or permissions. Those are production rules: they
make a fresh export deterministic, and they carry no record content.
An archive whose entries and manifests are intact is still a verified
export after a tool has repacked it, which is what an operator asking
"are these records intact?" needs to hear. Proving an archive is
byte-identical to a fresh export is a byte comparison, not a
verification finding.

The container is unchanged by how an installation delivers it. A
producing installation writes the archive into the response as it is
produced, so a transfer that ends early leaves an incomplete file: it
lacks the central directory, the verifier reports it as unreadable rather
than as a smaller valid export, and the transfer ends in an error rather
than in the terminal chunk that would have made it look complete. A
complete transfer is an explicit fact the producer records only when the
whole archive was produced and its tail flushed; every other ending — a
production failure, a producer that panicked, a client that stopped
reading long enough to lose the failure signal — fails the body. The
installation holds no database transaction across the transfer: the
archive's scope is fixed when its manifest is read, and each unit's
bytes are read and checked against the manifest's stored hashes as they
are written, so a unit that no longer matches ends the transfer
incomplete. "The state at its recorded instant" therefore means the
scope and the fingerprints the manifest states, with each unit's
immutable bytes verified against them. The audit event
records the export the installation produced from the state at its
recorded instant; it is not a receipt for a completed download, so a
failed delivery is a transfer the operator repeats rather than a record
the installation un-writes.

## What the archive does not carry

- **Drafts.** An unfinalized record is not a record; scopes contain
  finalized versions only, and a scope with none is refused, never
  exported empty.
- **Acknowledgments, amendments, and signoff history.** They are
  separate records bound to versions; the trainee packet (#44 decision
  3) gathers them per enrollment, and the installation export
  (`docs/formats/installation-export.md`) carries them, and drafts,
  installation-wide. An amendment's existence is
  visible here only as a successor version's `predecessor_content_hash`.
- **Exporter identity.** Who exported what is the installation's
  audit trail (`record_exported`), not the artifact's business.
- **Signatures.** The future signed mode adds them beside the hashes
  without changing a record byte (`docs/records-integrity.md`).

## Authorization (behavior of the producing installation)

Export is a read in a portable shape, so it follows the read rules that
already exist rather than inventing parallel ones: a version or record
exports for whoever may read the record (workflow readers and the
trainee's own finalized record, ADR 0012); an enrollment exports for
whoever may read its training history; the whole installation exports
for holders of `export_records`. Every export is audited with actor and
subject and never with content.
