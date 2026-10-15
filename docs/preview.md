# Preview deployment

The demonstration site is <https://consolebook-preview.conary.io>, on the host
`remi`. It contains invented data and is separate from development
installations. This is a host runbook, not a production-readiness statement.
Credentials belong outside the repository; the browser first prompts for nginx
Basic Auth, then Consolebook sign-in.

## Installed layout

The preview is a stock installation. It was installed on 2026-10-15 by the
v0.2.0 release's `install.sh`, from the release tarball, after both were
checked against their attestations
([#95](https://github.com/FieldmouseWorks/consolebook/issues/95)):

```sh
install.sh --from consolebook-linux-x86_64.tar.gz --public-url https://consolebook-preview.conary.io --backup-dir /data/consolebook-backup-copies
```

The layout is exactly the one in [install.md](install.md):

| Component | Location or behavior |
| --- | --- |
| Program | `/usr/local/bin/consolebook` |
| Data and automatic backups | `/var/lib/consolebook` (on `/`, ext4) |
| Backup copies | `/data/consolebook-backup-copies` (a separate xfs filesystem) |
| Settings | `/etc/consolebook/consolebook.env`: `CONSOLEBOOK_DATA_DIR`, `CONSOLEBOOK_BIND=127.0.0.1:7770`, `CONSOLEBOOK_BACKUP_COPY_DIR`, `CONSOLEBOOK_PUBLIC_URL` |
| Service | `consolebook.service`, with the drop-in `consolebook.service.d/paths.conf` written by the installer |
| Service account | `consolebook` |
| nginx vhost | `/etc/nginx/sites-available/consolebook-preview` |
| Basic Auth file | `/etc/nginx/.htpasswd-consolebook-preview` |
| TLS certificate | `/etc/letsencrypt/live/consolebook-preview.conary.io/` |

The backup copy filesystem shares the host's NVMe devices. The copy protects
against filesystem damage, not against loss of the machine.

nginx terminates TLS, redirects HTTP to HTTPS, requires Basic Auth, adds HSTS
and no-index headers, and proxies to `http://127.0.0.1:7770` with `Host` and
`X-Forwarded-*` headers. It still sets
`proxy_cookie_flags consolebook_session secure httponly samesite=strict`. That
is redundant: under `--public-url` the app marks the session cookie `Secure`
itself ([ADR 0025](decisions/0025-public-url-and-secure-cookies.md)).

The installer writes a service unit that is less hardened than the earlier
hand-made one ([#125](https://github.com/FieldmouseWorks/consolebook/issues/125)).
That hand-made install (`consolebook-preview.service`, `/srv/consolebook-preview`,
the `consolebook-preview` account, last built 2026-09-01 and reporting `0.0.0`)
was removed in [#124](https://github.com/FieldmouseWorks/consolebook/issues/124).

Merging a PR does not deploy the preview; there is no automatic deploy pipeline. The
installed version and source commit appear in the footer,
`consolebook --version`, and `/api/health` (ADR 0022).

## Read-only status checks

On the host:

```sh
systemctl status consolebook --no-pager
curl --fail http://127.0.0.1:7770/api/health
curl --head https://consolebook-preview.conary.io/
sudo -u consolebook CONSOLEBOOK_DATA_DIR=/var/lib/consolebook consolebook doctor --backup-copy-dir /data/consolebook-backup-copies
journalctl -u consolebook
```

The health response reports `version`, `commit`, and `database`; the database
should be `ok`. An unauthenticated HTTPS request should receive `401` with a
Basic Auth challenge. `doctor` is read-only
([ADR 0016](decisions/0016-read-only-diagnostics.md)). None of these checks
proves recovery or a complete user workflow.

First-run logs and installer output can contain the short-lived setup code.
Inspect them privately and never paste them into issues.

## Updating the preview

Use the issue/branch/PR workflow in [CONTRIBUTING.md](../CONTRIBUTING.md) for
deployment changes. Check the current host configuration and data state first;
this runbook does not authorize replacing or reinitializing retained data.

Upgrade by following [Upgrading](install.md#upgrading) in install.md: rerun
the installer, optionally with `--version X.Y.Z`. A release with a migration
takes a validated backup first. After upgrading, verify service state,
loopback health, HTTPS authentication, and a browser sign-in through the
public hostname. Record the executable SHA-256 and the version from
`/api/health` with each deployment; deployments are tracked in an issue.

An older binary is not necessarily compatible with a migrated database. Do not
assume a binary-only rollback is safe; go back the way
[Upgrading](install.md#upgrading) describes, by installing the previous
version and restoring its pre-migration backup.

## Certificate renewal

The host uses Certbot's standalone authenticator. Existing global renewal hooks
stop nginx before renewal and start it afterward, briefly affecting all nginx
sites. A webroot challenge would fail with those hooks because its server is
stopped. Keep the authenticator and hooks consistent; do not change them for
one vhost without considering the other sites. Inspect the renewal config and
timer on the host; successful issuance alone does not prove a future renewal.

If Cloudflare proxying is enabled, use Full (strict) TLS to the origin. The
application remains on loopback; public port 7770 is unnecessary.
