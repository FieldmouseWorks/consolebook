# Changelog

Notable changes to Consolebook, by release. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[ADR 0022](docs/decisions/0022-product-versioning.md): Semantic Versioning,
pre-1.0, with portable formats and migrations versioned separately.

Consolebook is pre-alpha. A release is a reproducible, identifiable build,
not a production-readiness statement.

## [Unreleased]

### Added

- Before a new version migrates the database, it takes a validated backup
  of it as it was and refuses to migrate without one, whichever command
  runs first; the install guide shows how to go back with the previous
  release.
- A version that finds a database upgraded by a newer release refuses it in
  plain words and names the way back.
- `--public-url` / `CONSOLEBOOK_PUBLIC_URL` (installer: `--public-url`): the
  address people use. An `https://` address marks sign-in cookies `Secure`,
  and the sign-in page points browsers that arrived by another address to it
  (ADR 0025). The install guide gives a tested Caddy configuration for HTTPS.
  Turning it on signs out sessions whose cookies were not `Secure`.
- `doctor --backup-copy-dir` reports whether off-server backup copies are
  current, behind, unreachable, or not configured, without writing anything.
- [ADR 0027](docs/decisions/0027-backup-copy-encryption.md): Consolebook does
  not encrypt backup copies; the install guide states what the share must
  provide.
- A [restore drill](docs/restore-drill.md) from a lost server to verified
  records, with a scripted version (`scripts/restore-drill/`) and a recorded
  run from the v0.1.0 release.
- [Operating a pilot installation](docs/operations.md): what to monitor and
  how, the logs and their retention, reporting a problem, and what support a
  pre-alpha pilot can expect. The release workflow runs its commands against
  a real install.

### Changed

- The one-line install fetches the installer published with the latest
  release instead of the `main` branch, and every release asset carries
  build provenance (`gh attestation verify`; ADR 0026).

### Fixed

- A new server set up to replace a lost one no longer copies its own empty
  database into the backup copy directory as the newest backup. A copy
  directory holds one installation's backups, and copying there is refused
  until the lost installation is restored; a restore then backs up what it
  restored, so copies resume at once. The database a restore sets aside is
  never copied off the server ([#115](https://github.com/FieldmouseWorks/consolebook/issues/115)).
- Two backups or restores within the same second no longer fail or mislabel
  the set-aside database as damaged; the second waits for a new name.
- `doctor` judges whether the newest backup is overdue against the service's
  backup interval (`--backup-interval-hours`, or
  `CONSOLEBOOK_BACKUP_INTERVAL_HOURS`) instead of always 24 hours ([#117](https://github.com/FieldmouseWorks/consolebook/issues/117)).
- A phase change that waited behind another write is no longer refused as
  out of order; it is judged at the moment it can commit ([#108](https://github.com/FieldmouseWorks/consolebook/issues/108)).
- After a restore, the set-aside database no longer counts as the newest
  backup: snapshots order by their time, so off-server copy status,
  catch-up copies, and retention follow the latest backup ([#105](https://github.com/FieldmouseWorks/consolebook/issues/105)).
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
