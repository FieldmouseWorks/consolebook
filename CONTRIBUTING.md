# Contributing

Consolebook is pre-alpha. Follow [AGENTS.md](AGENTS.md) and the product
constraints in [PRINCIPLES.md](PRINCIPLES.md). The task index and source map in
[docs/development.md](docs/development.md) route to additional context.

## Issue, branch, and pull-request workflow

Non-trivial implementation, bug, refactor, documentation, operations, and
maintenance work uses one primary GitHub issue, an issue-linked branch, and a
pull request. Never push repository changes directly to `main`.

- Inspect the checkout and existing PR reviews; preserve unrelated edits.
- Search open and closed issues before filing a new one.
- Discuss broad domain or architecture changes before writing a large patch.
- `Closes #...` means the PR satisfies the issue's acceptance criteria; use
  `Refs #...` for a slice that leaves the issue open.
- Commit subjects follow the house style in [AGENTS.md](AGENTS.md#commits).
- Security reports use private advisories per [SECURITY.md](SECURITY.md).

GitHub enforces the following; the `main` ruleset is the rung-2 guard for
"never push directly to `main`":

- `main` changes only by squash-merged pull request with linear history, every
  review conversation resolved, and no force push or deletion. Administrators
  bypass only by merging a pull request, never by pushing.
- Required checks, on a branch up to date with `main`: `check / check`
  (`flake.yml`, `nix flake check`), `fmt-clippy-test` (`pr-gate.yml`; the name
  is kept although fmt and clippy now run in the flake), and `build-x86_64` and
  `build-aarch64` (release build jobs).
  Renaming or removing these jobs, or changing the build matrix, leaves merges
  waiting on a check that never reports; update the ruleset in the same change.
- Tags `v*` cannot be deleted, updated, or moved, and published releases are
  immutable, so the release draft-upload-then-publish order is load-bearing.
- Actions must be GitHub-owned, or one of the three allowlisted: for the flake
  gate, burrow's shared `flake-check.yml` and `cachix/install-nix-action`, and
  for the house rules, burrow's `house` action. Every `uses:` is pinned to a
  full commit SHA; anything else fails before it runs. Secret scanning with push protection and CodeQL
  code scanning are enabled.

Dependencies move to their latest stable release where it makes sense; any
hold is written down with its reason, as the `.github/dependabot.yml` ignore
rules are. Dependabot proposes grouped weekly version updates after a 7-day
cooldown, and security updates as advisories appear.

Use [the evidence-driven workflow](docs/workflow.md) for outcome definition,
roles and ownership, task graphs, evidence-driven continuation, and authorization.
The primary issue body owns the live graph; issue comments archive completed
snapshots and receipts. PRs link that record. Simple changes use a short plan.
The [roadmap](docs/roadmap.md) is the current-state entry point, not a second tracker.

## Engineering discipline

Fix causes and prove the contract, including in-scope defects and duplicated
authority. File exact-evidence issues for separate work. Heuristics may assist
discovery or presentation; authorization, integrity, retention, disposition,
and exports require typed contracts. Investigate intermittent failures rather
than retrying until green.

Keep modules focused on a domain capability. For Rust source files:

- Adding behavior to a file over 1,000 lines requires an ownership-based
  reorganization in the same issue or plan. Thin dispatch, registration, and
  re-export wiring may remain in a large hub.
- Before changing behavior in a file over 1,500 lines, name the ownership
  boundary being preserved or improved.
- Files over 2,500 lines require a reviewed decomposition path before major
  feature work unless the fix is urgent.
- Refactors name what moves, its new owner, persisted/public impact, and the
  focused proof. Avoid unrelated rewrites.

### Correct the class, not the instance

Agents and people extend the patterns they read, so the codebase is the most
durable instruction in the repository. Whenever a review, a failing check, or a
corrected agent exposes a recurring mistake, fix the class at the strongest rung
that applies, and name the rung in the PR:

1. **Codebase.** Make the mistake unrepresentable: a type, an exhaustive match,
   one owning API, a schema constraint or trigger.
2. **Mechanical enforcement.** Enforce it with the compiler, clippy, a test
   with a negative control, or a repository guard the gates run.
3. **Guidance.** Write it into `AGENTS.md`, this file, or the owning document —
   knowing guidance can be skipped.
4. **Review.** Rely on a reviewer noticing only when nothing stronger applies;
   at any volume this is a hole, not a control.

Keep one paved path per concern. A workaround, a duplicate path, or a comment
that justifies either is a defect: each copy makes the next copy likelier.
When an anti-pattern cannot be removed in the current slice, first add the
guard that stops new instances, then file the cleanup with exact evidence. The
gardening — deleting debt, keeping the single path, guarding against drift — is
part of ordinary work, not a separate project.

Decisions changing durable behavior get an ADR in `docs/decisions/`; changes
to `PRINCIPLES.md` require one. Schema changes use forward migrations. Portable
format changes update the specification, producer, verifier, and fixtures
together, with versioning governed by that format's contract.

## Build and verification

Use the Rust toolchain pinned in `rust-toolchain.toml` and npm's committed
lockfile. `web/package.json` declares Node.js `>=24`, and `@types/node` follows that
floor; CI builds with Node.js 24.
Node.js is build-time only.

From the repository root, run in this order:

```sh
scripts/check-guidance-names.sh
(cd web && npm ci && npm run check && npm run build)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p consolebook-server
(cd web && npx playwright install chromium && npm run e2e)
```

`cargo xtask proof` (`crates/xtask`) runs this exact block, one line per step
from the repository root, stops at the first failure, and writes a local
receipt under the ignored `.proof/` (`--chromium PATH` uses the system-browser
line below; `--list` prints what it will run). The block is therefore
executable input: keep one command per line, and keep exactly one line running
`npm run e2e` and one documented `CONSOLEBOOK_E2E_CHROMIUM=/path/to/chromium`
alternative, or the runner refuses to run. The
[consolebook-proof skill](.agents/skills/consolebook-proof/SKILL.md) routes a
change to its focused proof before this full run.

`scripts/check-guidance-names.sh` is the rung-2 guard for one recurring
class: tracked agent guidance naming specific models or vendors instead of
roles. Its scanned set, pattern list, and entrypoint-name allowlist live at
the top of the script, with instructions for extending them.

The explicit build supplies `target/debug/consolebook-server` for the browser
tests. The shared fixture starts a scratch installation per test on an
OS-assigned loopback port and waits for shutdown before deleting its data.
`npm run check` checks both Svelte code and browser-test TypeScript.
If a system Chromium is available, skip the browser download and run:

```sh
(cd web && CONSOLEBOOK_E2E_CHROMIUM=/path/to/chromium npm run e2e)
```

Linux browser dependencies can be installed with
`npx playwright install --with-deps chromium` from `web/`.
Run Unix diagnostic permission tests as an unprivileged user; they assert that
filesystem permissions deny writes, which a privileged runner can bypass.
The [pr-gate workflow](.github/workflows/pr-gate.yml) runs the web build and
check, the Rust tests against the embedded interface, and the browser suite on
pull requests, and the [release workflow](.github/workflows/release.yml)
builds the static Linux archives and installs and upgrades them on a systemd
runner. The [flake workflow](.github/workflows/flake.yml) runs `nix flake check`
([flake.nix](flake.nix)): format, clippy, Rust tests without embedded web
assets (the proof runner's included), Python syntax, and the guidance guard,
each in the Nix sandbox. The [house workflow](.github/workflows/house.yml)
holds each pull request to the house rules a machine can read, its title and
body included. Focused local tests help development but do
not replace required gates. Preserve exact failure evidence and report only
commands actually run; an unexplained failure is not a reason to retry.

## Documentation and fixtures

State what is implemented, what is proposed, and what was verified. Link to
the owning specification or decision instead of duplicating it. Update the
roadmap when milestone state changes and the development map when ownership
moves. Do not put session transcripts, transient branch inventories, or
verification claims without a named revision into agent entrypoints.

For documentation changes, review the diff, run `git diff --check`, check changed
relative links and heading anchors, and validate any edited issue-template YAML.
Compare documented commands with package scripts and the existing PR gate.
When changing agent instructions, inspect the applicable global/project chain
including any tool-specific override or nested instruction file, and check a fresh session when the host supports
it; otherwise record that limitation. These checks supplement the required
build and browser gates above; there is no documentation-only exemption.

All fixtures, screenshots, examples, and seed data use invented agencies,
people, incidents, identifiers, narratives, and schedules. Real operational
material and credentials never enter the repository or public issue text.
Remove sensitive values from diagnostic output before sharing it.

## Licensing contributions

Consolebook is licensed under AGPL-3.0-only. Contributions use the same terms.
The project requires neither a separate contributor license agreement nor a
broad relicensing grant.
