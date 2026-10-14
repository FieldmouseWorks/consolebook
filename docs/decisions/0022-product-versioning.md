# ADR 0022: Product versioning and build identity

- **Status:** Accepted
- **Date:** 2026-10-14

## Context

Every surface reported `0.0.0`: the CLI, `/api/instance`, `/api/health`, the
startup log, and the interface footer. The preview runbook recorded the cost:
the reported version "cannot identify which commit is running", so each
deployment had to note its source commit and binary hash by hand. There were
no tags, no changelog, and no statement of what a version number promises.

Consolebook already versions several things on their own terms, each with its
own compatibility contract: the canonical record schema (ADR 0011), the record
export and trainee packet formats (ADR 0014, ADR 0015), program version
exports (ADR 0007), and the database through forward migrations. A product
version must not blur those.

## Decision

### Semantic Versioning, pre-1.0

The product version is `MAJOR.MINOR.PATCH` under
[Semantic Versioning](https://semver.org/). While pre-alpha it stays at `0.y.z`:

- **minor** (`0.y.0`) for new capabilities, any new database migration, or any
  change an operator must act on (configuration, CLI, a portable format
  version);
- **patch** (`0.y.z`) for fixes and presentation changes that need no new
  migration and no operator action.

`1.0.0` is reserved for the production-readiness decision that ends
Milestone 6. Each portable format and the record schema keep their own
version and compatibility rules; a product release states which ones it
produces and reads, and never replaces them.

### One source of truth

`[workspace.package] version` in the root `Cargo.toml` is the only place the
version is decided. `web/package.json` and its lockfile mirror it (set with
`npm version <v> --no-git-tag-version`), and `CHANGELOG.md` must have a
section for it. `tests/release_version.rs` fails the build when any of these
disagree, so drift is caught by the gates rather than by review.

### Build identity: version plus commit

`build.rs` records the source commit as `CONSOLEBOOK_BUILD_COMMIT`: the value
of that environment variable when a release build sets it, otherwise the
checkout's abbreviated `HEAD`, otherwise `unknown`. `--version` and the
startup log show `VERSION (COMMIT)`; `/api/instance` and `/api/health` report
`version` and `commit` separately; the footer shows both. A commit of
`unknown` means the binary cannot be traced to source and should not be
deployed. The commit reflects `HEAD`, not uncommitted edits; a local build
from a dirty tree is a development build, not a release.

### Releases and tags

A release is a merged pull request that sets the version and moves the
changelog's `Unreleased` entries under the new version heading. After merge,
the merge commit on `main` is tagged `vX.Y.Z` (annotated). Tags are never
moved or reused. `docs/releasing.md` holds the procedure.

## Consequences

- Operators and the preview runbook can identify a running build from the
  footer or `/api/health` without out-of-band notes.
- Version bumps touch three files plus the changelog; the guard test makes a
  partial bump fail locally and in `pr-gate`.
- A build in a source archive without `.git` reports `unknown` unless the
  builder sets `CONSOLEBOOK_BUILD_COMMIT`; release automation must set it.
- Release artifacts and their automation are a separate decision and
  workflow; this ADR defines only what a version and a build identity mean.
