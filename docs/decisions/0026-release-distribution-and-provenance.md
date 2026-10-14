# ADR 0026: Release distribution and provenance

- **Status:** Accepted
- **Date:** 2026-10-14
- **Amends:** [ADR 0023](0023-linux-installation-lifecycle.md) (how the installer is fetched)

## Context

The documented one-line install piped `install/install.sh` from the `main`
branch. Every merge to `main` therefore reached every new installation
immediately, before any release was cut. The installer then fetched an
archive and its SHA-256 from the same GitHub release. That checks for
corruption, but it is not evidence of origin: whoever can replace the
archive can replace its checksum. ADR 0023 left both as open decisions.

The options for origin evidence were:

- a long-lived signing key (minisign, GPG): the project would have to hold,
  rotate, and protect a key and publish its fingerprint;
- keyless provenance from the build itself: GitHub artifact attestations
  (Sigstore), which record that a file was built by a named workflow, in
  this repository, at a given commit.

## Decision

- **The installer ships with the release.** The release workflow publishes
  `install.sh` and `install.sh.sha256` as assets beside the archives. The
  documented command fetches
  `https://github.com/FieldmouseWorks/consolebook/releases/latest/download/install.sh`,
  so it runs the reviewed script that was released with the archives it
  installs. A test fails if any document, the interface, or a workflow
  points the command at `main` again.
- **Every release asset carries build provenance.** The release workflow
  attests each archive and the installer with
  `actions/attest-build-provenance`. Anyone can check an asset's origin with
  `gh attestation verify <file> -R FieldmouseWorks/consolebook`.
- **No long-lived signing key for now.** Provenance gives origin evidence
  without a key to hold, lose, or rotate. The installer keeps verifying the
  checksum, which it can do with only `sha256sum`. It does not require
  `gh` to check attestations, because that would add a dependency every
  server must install first.

## Consequences

- An unreleased change on `main` no longer reaches new installations; only
  a tagged release does.
- Origin can be verified by people who want to, with standard tooling. The
  default install trusts GitHub's release hosting over HTTPS plus the
  checksum, and the trust boundary is stated rather than implied.
- A pinned-key signature, and the installer verifying it by default, stay
  open. Revisit them if agencies need offline verification or an origin
  independent of GitHub.
- `--version X.Y.Z` still installs any published release with the latest
  installer.
