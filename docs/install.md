# Install Consolebook

This page takes a Linux server from nothing to a working Consolebook with your
first program, people, and training session. You need a Linux machine with
systemd (Ubuntu, Debian, RHEL, Rocky, Alma, Fedora, and similar), x86_64 or
arm64, and an administrator account that can use `sudo`.

Consolebook is pre-alpha. Use invented data until your agency has decided it is
ready for real records. [ADR 0024](decisions/0023-linux-installation-lifecycle.md)
records the locations, service account, and upgrade behavior this page relies on.

## 1. Install

First, decide where a **second copy of every backup** will live: another disk
in the server, or a network share (NAS) mounted on it, say at
`/mnt/backups/consolebook`, and mount it before installing. Consolebook backs
itself up automatically, but a backup on the same disk as the records dies with
that disk. Then run one command on the server:

```sh
curl -fsSL https://raw.githubusercontent.com/FieldmouseWorks/consolebook/main/install/install.sh | sudo sh -s -- --backup-dir /mnt/backups/consolebook
```

Leaving out `--backup-dir` still installs, but the installer ends with a
warning and the administrator's home keeps one up until copies are flowing.
There is no database server to install: the database is one SQLite file that
Consolebook creates, upgrades, and checks itself.

It downloads the latest release for your machine, checks its SHA-256, and
installs:

| What | Where |
| --- | --- |
| The program | `/usr/local/bin/consolebook` |
| Your data and automatic backups | `/var/lib/consolebook` (`--data-dir` to choose another) |
| A copy of every backup | the `--backup-dir` you gave |
| Settings | `/etc/consolebook/consolebook.env` |
| The service | `consolebook` (starts at boot) |

When it finishes, it prints an address and a **setup code**:

```text
Consolebook is running. Finish setup in a web browser:

  1. Open        http://127.0.0.1:7770/
  2. Setup code  9f2c41d07a6be835c1e04f9d2b7a6c13
  3. Name your agency and create the first administrator.
```

### Reaching it from other computers

By default Consolebook listens on the server itself only. To let browsers on
your network reach it, install with `--lan`:

```sh
curl -fsSL https://raw.githubusercontent.com/FieldmouseWorks/consolebook/main/install/install.sh | sudo sh -s -- --lan
```

That serves plain HTTP on port 7770. That is fine for a trial on a trusted
network. For real use, put HTTPS in front of it: run your agency's reverse
proxy (nginx, Caddy, IIS ARR, or a load balancer) on the same machine, forwarding
to `127.0.0.1:7770`, and keep the default listen address.

Other options: `--version 0.1.0` installs a specific release, `--listen
ADDRESS:PORT` picks your own address, and `--from FILE` installs from a
downloaded archive (useful without internet access; keep the `.sha256` file
beside it).

## 2. Finish setup in the browser

1. Open the address the installer printed.
2. Enter the setup code, your agency's name, and the first administrator's
   username, display name, and password.
3. Sign in as that administrator.

Lost the code, or it expired? Make a new one on the server:

```sh
sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook setup-code
```

## 3. First day: from empty to a training session

Your home page shows a **Getting started** checklist that links each step.
*Guided setup*, linked from it, does the first steps for you: it adds people in
a batch and prints their sign-in sheet, starts an editable program from an
invented example, and shows backup health. Or work through the list by hand:

1. **Add people.** Under *Add a person*, create a coordinator, your trainers,
   and a trainee. Each gets a one-time code; pass it to them in person. They
   open the sign-in page, follow *Use a password reset code*, and choose their
   own password.
2. **Build a program.** Open *Programs*, create one, and fill in its phases,
   competencies and tasks, a rating scale, and an evaluation form. Save the
   draft.
3. **Publish it.** A published version is frozen: records made under it keep
   pointing at exactly this configuration. Changes later become a new version.
4. **Enroll the trainee** in the published version, from the version's page.
5. **Assign a trainer** and **record a training session** on the enrollment
   page.
6. The trainer signs in, opens the session from their home page, and writes
   the daily evaluation. A coordinator reviews and finalizes it, and the
   trainee acknowledges it from *My records*.

## Upgrading

Run the same install command again. It replaces the program and the service
definition, keeps `/var/lib/consolebook` and your settings, restarts the
service, and prints the address. The database is upgraded on start, and
startup refuses to run against data it cannot verify.

When a new version changes the database, it first takes a validated backup
of it as it was, and starts only if that backup succeeded. The log names it:

```sh
journalctl -u consolebook | grep 'took a validated backup first'
```

There is no downgrade: an older version refuses a database a newer one has
changed. To go back after an upgrade, restore that backup with the release
that wrote it:

```sh
sudo systemctl stop consolebook
curl -fsSL https://raw.githubusercontent.com/FieldmouseWorks/consolebook/main/install/install.sh | sudo sh -s -- --version 0.1.0 --no-service
sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook restore /var/lib/consolebook/backups/consolebook-<stamp>.db
sudo systemctl start consolebook
```

## Moving the data directory

The installer will not point an installation that has records at a different
directory: the service would start a new, empty installation there and the
records would seem to vanish. Move them deliberately instead:

```sh
sudo systemctl stop consolebook
sudo mv /var/lib/consolebook /srv/consolebook        # keeps backups/ with it
curl -fsSL https://raw.githubusercontent.com/FieldmouseWorks/consolebook/main/install/install.sh | sudo sh -s -- --data-dir /srv/consolebook
```

The rerun records the new location, grants the service access to it, and
starts it; `doctor` checks the moved database.

## Backups and recovery

Consolebook takes a validated backup automatically (every 24 hours by default,
keeping 14) into `backups/` inside the data directory. With a backup copy
directory, each validated backup is then copied there and checked byte for byte
before it counts (ADR 0024). Copies that fail are retried every 15 minutes and
reported to administrators on their home page. The administrator's home shows
when the newest backup ran and whether the copy is current.

To add or change the copy directory later, mount it and rerun the installer:

```sh
curl -fsSL https://raw.githubusercontent.com/FieldmouseWorks/consolebook/main/install/install.sh | sudo sh -s -- --backup-dir /mnt/backups/consolebook
```

On a network share, make sure the `consolebook` account can write it (for
example a CIFS mount with `uid=consolebook`), and mount it at boot (`/etc/fstab`
or a systemd mount unit). The copy directory must already exist on another
filesystem than the data directory: the installer refuses it otherwise, and
never creates it. If the share is not mounted later, its mount point is just an
empty local directory; Consolebook refuses to copy there, keeps backups local,
and reports the copy location as unreachable until the share is back. Copies are plain SQLite files: protect the share the way
you protect the server.

```sh
sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook backup    # take one now
sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook doctor    # read-only health check
```

Restoring replaces the database, so stop the service first:

```sh
sudo systemctl stop consolebook
sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook restore /var/lib/consolebook/backups/<snapshot>
sudo systemctl start consolebook
```

If the server's disk is gone, install on a new machine with the same
`--backup-dir`, stop the service, and restore the newest file from the copy
directory the same way.

## Day to day

| Task | Command |
| --- | --- |
| Is it running? | `systemctl status consolebook` |
| Logs | `journalctl -u consolebook` |
| Which version? | `consolebook --version`, or the footer of any page |
| Locked out administrator | `sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook recover --username NAME` |
| Change the listen address | edit `CONSOLEBOOK_BIND` in `/etc/consolebook/consolebook.env`, then `sudo systemctl restart consolebook` |

## Removing it

```sh
sudo systemctl disable --now consolebook
sudo rm /etc/systemd/system/consolebook.service /usr/local/bin/consolebook
sudo systemctl daemon-reload
```

This leaves your records in `/var/lib/consolebook` and settings in
`/etc/consolebook`. Delete those only when your retention obligations allow
it.

## Without systemd

`--no-service` installs the program, account, and directories without a
service. Run it under whatever supervises processes on that machine:

```sh
sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook serve --bind 127.0.0.1:7770
```

Windows and macOS servers are not packaged yet.
