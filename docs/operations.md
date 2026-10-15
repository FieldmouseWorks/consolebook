# Operating a pilot installation

What an agency's administrator watches, where the logs are, and how to report
a problem. It assumes the standard install from the [install guide](install.md):
the `consolebook` service, data in `/var/lib/consolebook`, settings in
`/etc/consolebook/consolebook.env`. The examples use the default address
`127.0.0.1:7770`; use yours.

Consolebook is pre-alpha. Read [what support to expect](#what-support-to-expect)
before you rely on it.

## What to watch

Three things go wrong in ways people notice late: the service stops, backups
stop reaching their copy, or the disk fills. Check all three daily, or have
your monitoring system do it.

### Is it up?

```sh
curl -fsS --max-time 10 http://127.0.0.1:7770/api/health
```

A healthy service answers in well under a second with
`{"status":"ok", ..., "database":"ok"}`. `curl -f` exits non-zero on anything
else, and `--max-time 10` turns a server that stops answering into a failure
instead of a check that hangs, so a monitoring check can use the exit status
alone. The health endpoint needs no sign-in and reveals only the version,
build, and whether the database answers.
Behind a reverse proxy, check the public address too: the service can be up
while the proxy or its certificate is not.

```sh
systemctl is-active consolebook
```

`active` means the service is running. systemd restarts it when it fails
(`Restart=on-failure`), but a service that was stopped, or exited cleanly,
stays stopped, so check for `active` whatever the reason.

### Are backups reaching their copy?

```sh
sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook \
  consolebook doctor --backup-copy-dir /mnt/backups/consolebook
```

Use the copy directory from `CONSOLEBOOK_BACKUP_COPY_DIR` in
`/etc/consolebook/consolebook.env`; without one, leave out
`--backup-copy-dir`. `doctor` only reads, so it is safe while the service
runs. Each line starts with `ok`, `warn`, or `FAIL`, and it exits non-zero
only on `FAIL`. Look at two lines:

| Line | Healthy | Act on |
| --- | --- | --- |
| `backups` | `ok`: the newest backup is younger than the backup interval | `warn` that the newest is too old: automatic backups have stopped; check the logs |
| `backup copy` | `ok  ... current:` the copy directory holds the newest backup | `not_configured`, `unreachable` (share not mounted or not writable), or `behind` for more than an hour |

`doctor` judges the `backups` line against a 24-hour backup interval. If
you set `CONSOLEBOOK_BACKUP_INTERVAL_HOURS` to something else, pass the same
value: `--backup-interval-hours 6`.

Every other line should be `ok`. The `migrations` line compares each
migration the database records with this version's: its number, its
checksum, and whether it finished. A `FAIL` there names the migration and
means the service will refuse to start on this database; the line says what
to do next.

An administrator also sees backup health on their home page, and gets a
notice when a copy fails.

### Is the disk filling?

```sh
df -h /var/lib/consolebook
sudo du -sh /var/lib/consolebook/backups
```

With a backup copy directory, check its disk too:
`df -h /mnt/backups/consolebook`, with your directory in its place.

Backups need room for about `CONSOLEBOOK_BACKUP_KEEP` (14 by default) copies
of the database, both in `backups/` and in the copy directory. Keep at least
twice that free. SQLite cannot write to a full disk: sign-in and saving stop
working until space is freed.

## Logs

The service logs to the systemd journal:

```sh
sudo journalctl -u consolebook --since today       # today's log
sudo journalctl -u consolebook -p warning          # warnings and errors only
sudo journalctl -u consolebook -f                  # follow live
```

### What the logs contain

- Startup: version, build commit, installation id, data directory, listening
  address, whether cookies are `Secure` (and how many older sessions were
  signed out when HTTPS was turned on), the pre-upgrade backup when an
  upgrade changes the database, and warnings about the setup, such as no
  backup copy directory or no HTTPS public address.
- Backups: each snapshot's path and size, each verified copy and its SHA-256
  hash, and failures.
- First-run setup: the one-time setup code, printed until setup is complete
  (ADR 0004). It is useless once setup finishes, and its completion is
  logged as a user id.
- Errors from requests that failed inside the server.

### What they exclude

They exclude record content, evaluation narratives, names, usernames,
passwords, reset codes, session tokens, and client addresses. Consolebook
writes no access log. If you need one, your reverse proxy keeps it, and
it then holds client addresses and URLs.

### How long logs are kept

journald keeps the service's log with the rest of the system's, under the
system-wide limits. On most distributions it is persistent when
`/var/log/journal` exists, and is capped by size, not age. journald has no
per-service retention. To check and set the limits:

```sh
sudo journalctl --disk-usage
sudo mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\nStorage=persistent\nSystemMaxUse=1G\nMaxRetentionSec=1year\n' | \
  sudo tee /etc/systemd/journald.conf.d/retention.conf
sudo systemctl restart systemd-journald
```

Choose values your agency's log policy allows. The logs hold no record
content, so they do not fall under your records retention schedule.

## Reporting a problem

**Security problems go privately through the form named in
[SECURITY.md](../SECURITY.md), never to a public issue.**

For anything else, open an issue at
<https://github.com/FieldmouseWorks/consolebook/issues>. Include:

1. The version: `consolebook --version`, or the footer of any page.
2. What you did, what you expected, and what happened instead.
3. The `doctor` output.
4. The log lines around the time, from
   `sudo journalctl -u consolebook --since "2026-10-14 09:00" --until "2026-10-14 09:30"`.

Never paste real records, names, or narratives, even in a screenshot.
Describe them, or use made-up stand-ins, for example: "a trainer named
Jordan Example saved a daily evaluation for the invented session on
2026-10-14 and the page said 'did not save'". Read log lines before
pasting them; replace anything that identifies a person or a call.

**Remove every `setup_code=` value before pasting a log.** Until first-run
setup is complete, the log carries the setup code, and anyone who has it and
can reach the server can create the first administrator. Replace it with
`setup_code=REDACTED`, along with any password, reset code, or session
cookie that ended up in something you paste. If you cannot tell whether a
log is safe to share, report through [SECURITY.md](../SECURITY.md) instead.

## What support to expect

Consolebook is pre-alpha, maintained in the open, with **no support commitment**:
no response time, no on-call, no guarantee a reported problem is fixed in any
particular release. Reports are read and answered as time allows.

What a pilot can rely on:

- Releases are built and tested by a public workflow, installed on a real
  systemd host before they publish, and the [changelog](../CHANGELOG.md)
  names what changed.
- An upgrade takes a validated backup before it changes the database, and
  says how to go back.
- Finalized records are never changed in place, and `doctor`, `export verify`,
  and backup validation let you check that yourself.

What a pilot must provide:

- HTTPS in front of the service before real records (install guide).
- An off-server backup copy, checked as above, and a restore you have
  rehearsed.
- Someone who reads the administrator's notices and the checks above.
