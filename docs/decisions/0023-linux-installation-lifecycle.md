# ADR 0023: Linux installation lifecycle

- **Status:** Accepted
- **Date:** 2026-10-14

## Context

Until now, running Consolebook meant building it with Cargo and hand-writing a
service definition. `install/install.sh`, the release archives (ADR 0022), and
`docs/install.md` give operators one command that installs, upgrades, and runs
it as a service. The first installation makes the names and locations below
part of an operator-facing contract: backups, monitoring, firewall rules, and
runbooks will point at them. They need a decision of record, so changing them
later is deliberate and has a migration path rather than happening by drift.

## Decision

### Supported platform

Linux on x86_64 or aarch64, with systemd for the service. `--no-service`
installs the files only, for hosts that supervise processes some other way.
Other platforms build from source until they are packaged.

### Names and locations

| What | Where | Owner and mode |
| --- | --- | --- |
| Executable | `/usr/local/bin/consolebook` | root, 0755 |
| Service account | `consolebook` system user and group, no login shell | — |
| Data directory (`CONSOLEBOOK_DATA_DIR`) | `/var/lib/consolebook`, or `--data-dir` | `consolebook`, 0750 |
| Backup copy directory (`CONSOLEBOOK_BACKUP_COPY_DIR`, ADR 0024) | none, or `--backup-dir` | `consolebook`, 0750 |
| Settings | `/etc/consolebook/consolebook.env` | root, 0644 |
| Service | `consolebook.service` in `/etc/systemd/system/` | root, 0644 |
| Writable paths for the service | `consolebook.service.d/paths.conf` drop-in | root, 0644 |
| Default listen address | `127.0.0.1:7770` (`--lan` for `0.0.0.0:7770`) | — |

The settings file is the operator's: the installer creates it once and
afterwards changes only the line an option names (`--listen`, `--data-dir`,
`--backup-dir`). It refuses to change the data directory of an installation
whose current directory holds a database, because the service would start an
empty installation in its place; moving records is a manual step the install
guide documents. The unit file
is the product's: each install replaces it, so hardening and fixes reach
existing installations. Local changes to the unit belong in a systemd drop-in.

### Service hardening

The unit runs as the service account with `NoNewPrivileges`,
`ProtectSystem=strict`, `ProtectHome`, private `/tmp` and devices, and
`ReadWritePaths` limited to the default data directory. The service cannot
rewrite its own executable, unit, or settings. A setting that needs another
writable path (a chosen data directory, a backup copy directory) is granted
by the installer's `paths.conf` drop-in, which it rewrites from the settings
file on each install, never by widening the base unit.

### Install and upgrade

The same command installs and upgrades.
- It verifies the archive's SHA-256 before changing anything. A download
  without a checksum is refused; a local `--from` archive without one is
  installed with a warning.
- On upgrade it stops the service, replaces the executable and unit, keeps the
  data directory and settings, and starts the service again.
- Database migrations are forward-only and run when the new executable starts
  (ADR 0003). Startup fails closed on any storage problem, and the installer
  reports failure if the service does not answer its health check within 30
  seconds.
- The installer probes the address the service is bound to, or loopback when
  it listens on every interface.

### Downgrade

There is no downgrade path. An older executable started against a database
migrated by a newer one finds applied migrations it does not know and refuses
to start, rather than running against a schema it does not understand.
Recovery from a bad upgrade is restoring the backup taken before the
database is migrated (ADR 0006) with the matching older release. Every
command that migrates (`serve`, `setup-code`, `recover`) takes that backup
first and refuses to migrate without it.

### Changing these later

Moving a location, renaming the account, or changing the default address is
a breaking operator change. Under ADR 0022 it requires a minor version, a
changelog entry naming the operator action, and an installer that migrates
an existing installation or refuses with instructions. It never strands data.

## Consequences

- Operators can script around fixed, documented paths, and an upgrade never
  touches their data or settings.
- HTTPS is not part of the installation: the service speaks plain HTTP behind
  a TLS reverse proxy. The installer's `--public-url` records the `https://`
  address, which makes session cookies `Secure` (ADR 0025); the install
  guide gives the supported proxy configuration.
- An upgrade that changes the database takes a validated backup first,
  whichever command migrates, and does not migrate without it. That backup is
  the recovery point for going back with the previous release.
- The one-line command fetches the installer from `main` and the archive from
  the release. Pinning the installer to the release and signing archives are
  open decisions.
