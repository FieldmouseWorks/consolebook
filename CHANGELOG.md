# Changelog

Notable changes to Consolebook, by release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[ADR 0022](docs/decisions/0022-product-versioning.md): Semantic Versioning,
pre-1.0, with portable formats and migrations versioned separately.

Consolebook is pre-alpha. A release is a reproducible, identifiable build,
not a production-readiness statement.

## [Unreleased]

### Added

- Before a new version migrates the database, `serve` takes a validated
  backup of it as it was and does not start without one; the install guide
  shows how to go back with the previous release.
- A version that finds a database upgraded by a newer release refuses it in
  plain words and names the way back.

### Fixed

- A draft that saves after a failed save no longer keeps saying "The draft
  did not save" ([#83](https://github.com/FieldmouseWorks/consolebook/issues/83)).

## [0.1.0] - 2026-10-14

The first versioned release: everything built through Milestone 4 and the
first Milestone 5 slices ([roadmap](docs/roadmap.md)).

### Added

- One executable and one SQLite data directory: first-run setup code, local
  administrator, sign-in, administrator-issued password resets, local
  recovery, `doctor` (read-only diagnostics), validated backups and restore.
- Versioned program configuration: programs, immutable published versions,
  phases and transitions, competencies, tasks, rating scales, evaluation
  forms, comparison, export and import.
- Training workflow: users and capabilities, enrollments and phase history,
  assignments, training sessions in agency-local time, daily evaluation
  drafts with autosave and attribution, review and change requests, notices.
- Defensible records: canonical record format, immutable finalized
  versions with content and chain hashes, acknowledgments, responses and
  refusals, amendments as successor versions, weekly summaries, task
  signoffs, and a trainee's own record timeline and signoff history.
- Exports: verifiable record exports and trainee packets, checked offline
  with `consolebook-server export verify`.
- Retention administration: explicit authority, versioned retention
  policies, and attributed record holds. Disposition is not yet executed.
- Build identity: `--version`, `/api/instance`, `/api/health`, the startup
  log, and the interface footer report the version and source commit.
- One-command install and upgrade for Linux with systemd
  (`install/install.sh`), static x86_64 and arm64 release archives, and an
  operator guide ([docs/install.md](docs/install.md)).
- A "Getting started" checklist on the administrator's home that follows the
  installation from first run to its first finalized evaluation, and an
  optional guided setup: people in a batch with a printable sign-in sheet,
  and an editable example program.
- Backup copies off the server (`--backup-dir` / `CONSOLEBOOK_BACKUP_COPY_DIR`,
  ADR 0024): each validated backup is copied to a second disk or share and
  verified byte for byte; backup health on the administrator's home, with a
  warning until copies are current; `--data-dir` to place the data.
- An interface that leads each role with its own work, shows who is signed in
  with sign out on every page, works at phone width, and explains refused or
  missing pages in plain words.
