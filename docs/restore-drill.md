# Restore drill: from a lost server to verified records

A drill proves what backups promise: after the server is gone, a new
installation holds the same records, unchanged. Run one before a pilot goes
live, after each upgrade to a new minor version, and once a year. Use test
machines and invented records, never the production server or real records.

The drill follows the install guide's
[backups and recovery](install.md#backups-and-recovery) steps exactly, so a
step that fails here would fail on the day it matters.

## What you need

- Two test machines or virtual machines that can mount the same backup copy
  directory. Host A stands in for the server you lose, and host B for its
  replacement.
- The Consolebook release you run in production, with its `.sha256` file,
  from the GitHub release page.
- About 30 minutes.

## The drill

### 1. Build the installation you will lose (host A)

1. Install the release with a backup copy directory, as in the install
   guide (`--backup-dir /mnt/backups/consolebook`).
2. Complete setup with an invented agency and administrator.
3. Create invented records:
   - a program, a trainee, and a trainer;
   - three daily evaluations, finalized;
   - the trainee's acknowledgment on two of them, one with a response.
4. Write down the evidence you will compare against. For each finalized
   record, note its version, content hash, chain hash
   (`GET /api/drafts/<id>/version`), and its acknowledgment. Download the
   installation's record export and each record's export.
5. Take a backup now and confirm that it reached the copy directory:

   ```sh
   sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook backup --backup-copy-dir /mnt/backups/consolebook
   sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook doctor --backup-copy-dir /mnt/backups/consolebook
   ```

   `doctor` must show `ok    backup copy    current: …`.

### 2. Lose it

Power off host A and delete it, or wipe its disk. From here on, use nothing
from it: only the copy directory and the evidence you wrote down.

### 3. Recover on host B

Follow the install guide's recovery steps ("If the server's disk is gone"):

1. Mount the copy directory and install the same release with the same
   `--backup-dir`. The new installation refuses to copy into the directory,
   because it holds another installation's backups, and logs why:

   ```sh
   sudo journalctl -u consolebook | grep 'backup copy failed'
   ```

   That refusal keeps your copies from being buried under the new, empty
   database. Check that the copy directory still holds only host A's copies
   (`ls -lt`).
2. Stop the service, restore the newest copy, and start the service again.
   The restore prints host A's installation id.
3. Run `doctor` with the copy directory. Every line must be `ok`, and the
   backup copy line must become `current`, naming the backup the restore
   took:

   ```sh
   sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook doctor --backup-copy-dir /mnt/backups/consolebook
   ```

### 4. Check the records

1. Sign in with host A's administrator. Accounts come back with the records.
2. For each record you wrote down, compare its version, content hash, chain
   hash, and acknowledgment. Each must be identical.
3. Verify every export from the files alone: the ones you downloaded on
   host A, and the same exports downloaded again on host B.

   ```sh
   consolebook export verify installation.zip
   ```

   Each must report that the export is consistent with its stated
   fingerprints.

Record the date, the release, timings, outputs, and anything that failed or
surprised you. File each failure as an issue, without real data.

## The scripted drill, for developers

`scripts/restore-drill/run.sh` runs the same drill on one Linux machine.
Two data directories stand in for the two hosts, and the copy directory is
on `/dev/shm`, another filesystem. `cargo xtask drill` (`crates/xtask`)
creates the invented records over the HTTP API and collects the evidence:
version metadata, acknowledgments, and exports. The script builds it first,
so it needs the repository's Rust toolchain. The administrator's password is
made for each run and kept in the work directory's `account.json`.

```sh
scripts/restore-drill/run.sh consolebook-linux-x86_64.tar.gz consolebook-linux-x86_64.tar.gz.sha256 /tmp/drill [HOST_B_BINARY]
```

The work directory must be new or empty. Without `HOST_B_BINARY`, host B
installs the same release. With it, host B runs that build, which drills
recovery into a newer version. The script exits 0 and prints `DRILL PASSED`
only when:

- the new server refuses to copy into host A's copy directory;
- restoring the newest copy brings back every record, acknowledgment, and
  export unchanged;
- `doctor` reports every line `ok`.

With v0.1.0 as host B, it fails on all three, as #115 did. The script is
no substitute for the drill on real machines: it has no systemd, no network
share, and no disk that fails.

## Recorded runs

### 2026-10-15: scripted, host A on v0.1.0

Host A ran the published v0.1.0 archive. Its sha256,
`87a6b3d2239fcfde49a19cd73f0718ad13a7b28b7f1cbca645cae1d0afb99239`,
was verified. The copy directory was on tmpfs and the data on ext4.

**Run 1: host B on v0.1.0. Failed.** The install guide's recovery steps
restored an empty database:

```
host A copies   consolebook-20261015T003221Z.db  consolebook-20261015T003222Z.db
host B starts   automatic backup complete and validated ... consolebook-20261015T003223Z.db size_bytes=552960
                backup copied off the server and verified copy=.../consolebook-20261015T003223Z.db
restore newest  restored .../consolebook-20261015T003223Z.db (installation 5ffe5235-…)
sign in         POST /api/auth/login: 401 invalid_credentials
```

The new installation had copied its own empty database into the copy
directory as the newest backup. The same run showed two more problems:
- v0.1.0 also copied the database a restore set aside off the server;
- two restores within one second labelled the set-aside database
  `.damaged`.

All three were fixed in
[#115](https://github.com/FieldmouseWorks/consolebook/issues/115).

**Run 3: host B on the build with that fix. Passed in 7 seconds.** This is
`scripts/restore-drill/run.sh`, with paths shortened:

```
host A copies   consolebook-20261015T013730Z.db  consolebook-20261015T013731Z.db
host B starts   ERROR backup copy failed: backup copy directory … holds the backups of
                installation 67789a46-…, and this is installation …; copying here would bury them …
copies after    consolebook-20261015T013730Z.db  consolebook-20261015T013731Z.db   (unchanged)
restore newest  restored …/consolebook-20261015T013731Z.db (installation 67789a46-…)
                backed up the restored database as …/consolebook-20261015T013735Z.db
doctor          ok    migrations             16 applied, up to date
                ok    integrity              ok
                ok    instance identity      67789a46-…
                ok    initialization         initialized, 1 administrator(s)
                ok    backups                3 snapshot(s), latest consolebook-20261015T013735Z.db
                ok    backup copy            current: … has 3, newest consolebook-20261015T013735Z.db
compare         instance equal (public_url is new in this version) | records: 3 vs 3
                record 1: content 1011bb41d385f76a chain e07bb0303452b555 meta equal ack equal
                record 2: content 75d860de7e496703 chain 5c5f6f15389be45b meta equal ack equal
                record 3: content 0512bd6738b371e0 chain 07ddc421e53448c6 meta equal ack equal
export verify   installation.zip and record-1/2/3.zip, before and after: each reports
                "the export is consistent with its stated fingerprints"
```

The restore also migrated the 0.1.0 database to the newer schema, from 15
migrations applied to 16.

Run 2 does not count. It used an earlier version of the script, which
started host B within the same second as host A's last backup, so host B's
first backup took the same file name. It passed without ever reaching the
refusal. The script now waits first, as a real replacement would.

Not yet drilled: real hosts under systemd with a network share. Run that
drill before a pilot, and record it here. Running the drill on every release
build is [#119](https://github.com/FieldmouseWorks/consolebook/issues/119).
