# ADR 0024: Backup copies off the server, and backup health

- **Status:** Accepted
- **Date:** 2026-10-14
- **Amends:** [ADR 0006](0006-backup-scheduling-and-restore.md)

## Context

ADR 0006 made backups automatic: a validated `VACUUM INTO` snapshot on a
schedule, pruned to a retention count, in `backups/` inside the data
directory. That directory shares a disk, and usually a failure domain, with
the database it protects. A failed disk, ransomware, or deleting the data
directory loses the records and every backup at once. Nothing in the product
asked anyone to keep a copy elsewhere, and nothing showed an administrator
when backups last ran. For training records under retention obligations, an
off-server copy has to be the expected state, not an afterthought.

## Decision

### An optional copy directory

`serve` and `backup` accept `--backup-copy-dir` /
`CONSOLEBOOK_BACKUP_COPY_DIR`: a directory on another disk or a mounted
network share. Consolebook does not speak network protocols; the operating
system mounts the target and Consolebook writes files. The setting is
optional, because an installation must still start and back up locally when
the share is down, but the product presses for it (below).

### The copy contract

After a local snapshot validates (ADR 0006), and only then:

1. refuse when the copy directory is missing or not a directory, is on the
   same filesystem as the local snapshots (the empty mount point of an
   unmounted share looks exactly like that), or already holds a copy of the
   same name (copies are never overwritten);
2. copy to `<name>.partial`, fsync it, and verify it **byte for byte**
   against the local snapshot by SHA-256 read back from the copy; a mismatch
   removes the partial file;
3. rename into place and fsync the file and its directory; if that fails,
   remove the copy so it is not counted and is made again; and
4. prune the copy directory to the same retention count, by the same rule:
   oldest first, never the last remaining copy.

The copy is verified by content, not by opening it as a database. The local
snapshot already passed SQLite's integrity check, and a byte-identical copy
is equally valid, without running SQLite on a remote filesystem or leaving
journal sidecars there. The read-back goes through the operating system and
may be answered from its cache: it proves what was written, not the medium.

A failed copy never removes, modifies, or invalidates the local snapshot. It
is logged and raised to administrators as a `backup_copy_failed` notice,
deduplicated like `backup_failed`. The scheduler retries every 15 minutes,
copying the newest local snapshot whenever the copy directory lacks it, so a
share that was down, or one configured later, catches up without waiting for
the next backup.

### Health is read from the files

`GET /api/backups/status` (`manage_users`, enforced by the service) reports
the interval, retention, the local snapshot count and newest snapshot, and
the copy state: `not_configured`, `unreachable` (with the reason), `behind`,
or `current`. A copy directory on the local snapshots' filesystem is
`unreachable`, never `current`. A copy is **current exactly when the newest
local snapshot's file name is present in a copy directory on another
filesystem**. Snapshot names carry their UTC
timestamp, so the check needs no stored state and does not depend on clocks
or modification times on the share.

### The product presses for a copy

The installer takes `--backup-dir`, which must already be mounted (it
refuses a missing directory or one on the data directory's filesystem, and
never creates one), and ends with a prominent warning without it. `serve` logs a warning at start without a copy directory. The
administrator's home shows backup health, with a warning that stays until
copies are current. The Getting started checklist has a step that is done
only when they are, and the guided setup shows the status with the exact
command to fix it. The service cannot rewrite its own configuration (the
systemd unit allows writes only to its data and copy directories; ADR 0023),
so the browser explains and the installer configures, adding the copy
directory to the unit through a drop-in as ADR 0023 requires. Using the application is
never blocked by a missing copy: a trainer locked out because a share is down
would be a second incident, not a safeguard.

## Consequences

- An installation that follows the installer's advice survives the loss of
  its own disk with at most one backup interval of data lost.
- Copies inherit ADR 0006's limits: they are plain SQLite files, unencrypted
  at rest; protecting the share is the agency's responsibility until an
  encryption decision exists.
- `doctor` does not yet inspect the copy directory; the health read and
  notices cover it while the server runs.
- Restoring from a copy is the existing `restore <snapshot>` path, which
  accepts any snapshot file.
