# ADR 0029: doctor's migration verdict matches what serve will do

- **Status:** Accepted
- **Date:** 2026-10-15
- **Issue:** [#138](https://github.com/FieldmouseWorks/consolebook/issues/138)
- **Amends:** [ADR 0016](0016-read-only-diagnostics.md) (adds the migration
  verdict to the read-only diagnosis; the read-only contract is unchanged)

## Context

`doctor` exits non-zero when any check reports `fail` and zero otherwise.
Operators and monitoring read that exit status, so what each check counts
as a failure is a contract.

The migrations check compared migration versions only. That made its verdict
disagree with `serve` in both directions:

- **Doctor said ok, serve refused.** A database whose recorded checksum
  differed from this build's, or whose migration was recorded as unsuccessful,
  read `ok`. `serve` refused to start on it ("migration 16 was previously
  applied but has been modified"). Found while proving #136.
- **Doctor said fail, serve started.** Pending migrations read `fail`, yet
  `serve` applies them on start after a validated backup (ADR 0023). An operator
  who installs a new version and runs `doctor` before restarting was told the
  installation had failed.

## Decision

- **One owning judgment.** A single storage function judges an applied
  migration history the way startup does, and both `doctor` and startup's own
  newer-database check read through it. Its refusal list was derived from
  sqlx 0.9's migrator: a version this build does not know, a row recorded as
  unsuccessful, and a checksum that differs from this build's. One refusal is
  this product's own: a database that already holds Consolebook tables but has
  no migration history, because migration 1 cannot run over existing tables.
- **`fail` means the service will refuse to start.** Each refusal is a `fail`
  finding that names the migration and says what to do, so `doctor` exits
  non-zero exactly where `serve` would refuse.
- **`warn` means the service will act on its own next start.** Pending
  migrations are a `warn`: the service takes a validated backup, then
  migrates. A pristine database with no history yet is also a `warn`: it holds
  no tables, and the service creates the history on first start. Neither
  changes the exit status.
- **`ok` means nothing is pending and nothing would be refused.**
- **The diagnosis stays read-only, as ADR 0016 requires.** It runs SELECTs on
  the read-only connection, and tests prove the database and its sidecars are
  byte-identical afterwards.
- **Real-binary parity tests (`tests/doctor_migrations.rs`) are the guard.**
  For each case they run `doctor` and `serve` on the same database, so the two
  verdicts cannot drift apart unnoticed.

## Consequences

- **Exit status changes for monitoring.** A monitor that alerted on
  `doctor`'s exit status during the window between installing a new version
  and restarting no longer alerts. A monitor that needs to see pending
  migrations must read the `warn` line, not the exit status.
- **A tampered or foreign migration history now fails `doctor` before a
  restart attempts it.**
- **Startup itself still delegates part of the judgment to sqlx.** The
  unfinished-row and changed-checksum refusals come from sqlx's migrator at
  startup. Because sqlx applies pending migrations in order while validating,
  startup can change a database before it refuses one. #142 tracks moving
  startup onto the same pre-check, so that a refusal changes nothing. When it
  lands, this ADR's "one owning judgment" covers startup completely.
