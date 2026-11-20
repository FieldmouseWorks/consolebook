# Releasing

How to cut a Consolebook release. [ADR 0022](decisions/0022-product-versioning.md)
owns what a version means; this page is the procedure. Releases follow the
normal issue, branch, and pull-request workflow in
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Choose the number

Pre-1.0, a release that adds a capability, a migration, or anything an operator
must act on is a **minor** bump (`0.1.0` → `0.2.0`). Fixes and presentation
changes without a migration are a **patch** (`0.2.0` → `0.2.1`).

## Prepare the release pull request

1. Set `version` under `[workspace.package]` in the root `Cargo.toml`.
2. Mirror it into the web package: `(cd web && npm version X.Y.Z --no-git-tag-version)`.
3. Run `cargo build -p consolebook-server` so `Cargo.lock` records the version.
4. In `CHANGELOG.md`, move the `Unreleased` entries under
   `## [X.Y.Z] - YYYY-MM-DD` and leave an empty `Unreleased` section.
5. Run the full gates (`cargo xtask proof`). `tests/release_version.rs` fails if
   any of the three versions or the changelog heading disagree.
6. Open the PR as `release: X.Y.Z`, with the changelog section in the body.

## Tag after merge

Once the PR is merged and `main` is green, tag the merge commit. Pushing the
tag runs [the release workflow](../.github/workflows/release.yml): it checks
the tag against the Cargo version, builds static x86_64 and arm64 archives,
installs and upgrades each with `install/install.sh` on a systemd runner, and
only then publishes the GitHub release with this version's changelog section
as its notes. The release carries the archives, `install.sh` (which the
documented one-line install fetches), their checksums, and build provenance
for each (ADR 0026).

```sh
git fetch origin main
git tag -a vX.Y.Z -m "Consolebook X.Y.Z" <merge-commit>
git push origin vX.Y.Z
```

Never move, delete, or reuse a pushed tag; a mistake is fixed by the next
release. If the workflow fails, the release stays a draft: fix the cause on
`main` and release the next patch version.

To build an archive locally, build `web/` and then run
`scripts/package-release.sh x86_64-unknown-linux-musl x86_64` (needs the
target and `musl-tools`); `install/install.sh --from dist/consolebook-linux-x86_64.tar.gz`
installs it. Check the identity of a build from the tag:

```sh
cargo run -p consolebook-server -- --version   # X.Y.Z (<commit>)
```

## Verify the published release

After the release workflow publishes `vX.Y.Z` for `<merge-commit>`, verify the
published assets from a scratch directory before calling the release complete:

1. **Assets and immutability.** Confirm all six assets are published, the
   release is no longer a draft, and immutability is enabled:

   ```sh
   gh release view vX.Y.Z -R FieldmouseWorks/consolebook --json assets,isImmutable,isDraft
   ```

   Expected: `isDraft: false`, `isImmutable: true`, and six assets
   (`consolebook-linux-x86_64.tar.gz`, `consolebook-linux-aarch64.tar.gz`,
   `install.sh`, and a `.sha256` for each).

2. **Download and verify checksums and installer stamp.**

   ```sh
   work=$(mktemp -d)
   gh release download vX.Y.Z -R FieldmouseWorks/consolebook -D "$work"
   (cd "$work" && sha256sum -c consolebook-linux-x86_64.tar.gz.sha256 \
       consolebook-linux-aarch64.tar.gz.sha256 install.sh.sha256)
   grep -Fx 'RELEASED_VERSION="X.Y.Z"' "$work/install.sh"
   curl -fsSL https://github.com/FieldmouseWorks/consolebook/releases/latest/download/install.sh \
       -o "$work/latest-install.sh"
   cmp "$work/install.sh" "$work/latest-install.sh"
   ```

   Expected: every checksum reports `OK`, `install.sh` carries
   `RELEASED_VERSION="X.Y.Z"`, and `releases/latest/download/install.sh` is
   byte-identical to the tagged release's `install.sh`.

3. **Build identity.** Extract the archive for the host architecture and check
   its reported version and commit:

   ```sh
   tar -xzf "$work/consolebook-linux-x86_64.tar.gz" -C "$work"
   "$work"/consolebook-*/consolebook --version
   ```

   Expected: `consolebook X.Y.Z (<merge-commit>)`.

4. **Strict build provenance.** Verify every one of the six assets with the
   strict flags (the loose form without workflow, ref, digest, and runner
   checks accepts attestations from any workflow or commit in the repository):

   ```sh
   for asset in \
       consolebook-linux-x86_64.tar.gz \
       consolebook-linux-x86_64.tar.gz.sha256 \
       consolebook-linux-aarch64.tar.gz \
       consolebook-linux-aarch64.tar.gz.sha256 \
       install.sh \
       install.sh.sha256; do
     gh attestation verify "$work/$asset" \
       -R FieldmouseWorks/consolebook \
       --signer-workflow FieldmouseWorks/consolebook/.github/workflows/release.yml \
       --source-ref "refs/tags/vX.Y.Z" \
       --source-digest "<merge-commit>" \
       --deny-self-hosted-runners
   done
   ```

   Expected: all six pass verification against the release workflow, tag ref,
   and merge commit digest on GitHub-hosted runners.

5. **Negative controls.** Confirm the provenance check actually fails closed:

   ```sh
   # Tampered asset must find no matching attestation (fails / 404).
   cp "$work/install.sh" "$work/install-tampered.sh"
   printf '\n# tampered\n' >> "$work/install-tampered.sh"
   ! gh attestation verify "$work/install-tampered.sh" \
       -R FieldmouseWorks/consolebook \
       --signer-workflow FieldmouseWorks/consolebook/.github/workflows/release.yml \
       --source-ref "refs/tags/vX.Y.Z" \
       --source-digest "<merge-commit>" \
       --deny-self-hosted-runners

   # Wrong tag ref must be refused even for an unmodified asset.
   ! gh attestation verify "$work/install.sh" \
       -R FieldmouseWorks/consolebook \
       --signer-workflow FieldmouseWorks/consolebook/.github/workflows/release.yml \
       --source-ref "refs/tags/v0.0.0-wrong" \
       --source-digest "<merge-commit>" \
       --deny-self-hosted-runners
   ```

   Clean up the scratch directory (`rm -rf "$work"`) when finished.

6. **Preview deployment (separate, owner-approved).** Upgrading the live
   demonstration instance from the verified archive is a separate deployment
   requiring explicit owner approval; follow [docs/preview.md](preview.md).
