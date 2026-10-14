# ADR 0025: The public URL and `Secure` session cookies

- **Status:** Accepted
- **Date:** 2026-10-14
- **Amends:** [ADR 0004](0004-local-authentication.md) (session cookie attributes)

## Context

ADR 0004 delivers sessions in an `HttpOnly`, `SameSite=Strict` cookie and
deferred the `Secure` attribute to deployment hardening before any pilot.
Without it, a browser will send the session cookie over plain HTTP whenever
the installation is reached that way, so a misconfigured link or a
downgrade on a hostile network exposes a live session. The service itself
speaks plain HTTP; TLS is the job of a reverse proxy in front of it
(ADR 0023). The service cannot tell on its own whether a browser reached the
proxy over HTTPS. Forwarded headers such as `X-Forwarded-Proto` would say
so, but a client that reaches the service directly can forge them.

## Decision

An operator states the address people use: `serve --public-url` /
`CONSOLEBOOK_PUBLIC_URL`, set by the installer's `--public-url`. It is
parsed into a typed origin (`public_url::PublicUrl`): `https://` or
`http://`, a host, and an optional port. A path, query, fragment, or user
information is refused at startup, because Consolebook is served at the root
of its origin.

- **An `https://` public URL marks every session cookie `Secure`.** That
  covers the cookie set at sign-in and the one that clears it at sign-out.
  The decision comes only from this setting, never from request headers.
- Each session records whether its cookie was issued `Secure`. When `serve`
  starts with an `https://` public URL, it revokes every live session that
  was not: a browser may still hold that cookie and send it over plain
  HTTP, so it stops being a credential at once, not at its expiry. People
  signed in before the change sign in again, at the public address.
- Without a public URL, or with an `http://` one, cookies are not `Secure`,
  so a loopback install and a plain-HTTP `--lan` trial still work.
- `serve` logs the choice at start. When it listens beyond loopback without
  an `https://` public URL, it warns that cookies are not `Secure` and
  points to the install guide.
- `/api/instance` reports the public URL. The sign-in page compares it with
  the address the browser actually used. If they differ, the page names the
  right address, because otherwise the failure is silent: a browser drops a
  `Secure` cookie on plain HTTP, so sign-in would appear to succeed and then
  return to the sign-in page.

The supported front end is a reverse proxy on the same machine that
terminates TLS and forwards to `127.0.0.1:7770`. The install guide gives a
tested Caddy configuration and the requirements any other proxy must meet.

## Consequences

- Reached through HTTPS at its public URL, an installation never sends its
  session cookie over plain HTTP.
- A wrong or missing public URL degrades visibly, as a startup warning or a
  sign-in notice, never by silently weakening cookies or locking people out.
- HSTS, certificate management, and TLS settings belong to the proxy and
  are outside the product.
- The public URL is available for later features that need an absolute
  address, such as links in printed sign-in sheets.
