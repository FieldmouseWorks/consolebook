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
5. Run the full gates (`scripts/proof.py`). `tests/release_version.rs` fails if
   any of the three versions or the changelog heading disagree.
6. Open the PR as `release: X.Y.Z`, with the changelog section in the body.

## Tag after merge

Once the PR is merged and `main` is green, tag the merge commit:

```sh
git fetch origin main
git tag -a vX.Y.Z -m "Consolebook X.Y.Z" <merge-commit>
git push origin vX.Y.Z
```

Never move, delete, or reuse a pushed tag; a mistake is fixed by the next
release. Check the identity of a build from the tag:

```sh
cargo run -p consolebook-server -- --version   # X.Y.Z (<commit>)
```
