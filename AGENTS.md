# Repository Guidelines

Consolebook is pre-alpha training-record software for emergency communications
centers: one Rust executable, one SQLite data directory, and a static SvelteKit
UI. No required external services, telemetry, or production Node.js runtime.

<!-- house:rules v1 begin. Copied from FieldmouseWorks/burrow house/rules.md, verbatim. Change it there, not here: burrow's house sync rewrites everything between these markers. -->
## House rules

These hold in every repo. This file's own sections add to them and win where
they say something more specific.

- Fix the whole cause. No workarounds, shims, fallbacks or half-migrations:
  when you replace something, delete the old path in the same change.
- One fact, one home. Each rule or fact lives in one place; link to it instead
  of restating it.
- Before you edit, say what done looks like and how it will be checked. Simple
  work gets a short plan, not a ceremony.
- Report only the checks you actually ran, and what they said. Skipped,
  unavailable or "should pass" is never a pass. Keep local and CI results
  apart.
- A new check comes with proof that it fails on the bug it's for.
- Touching a file over about 1,000 lines? Split it in that change.
- The code is the memory. When something goes wrong, make the code, a type, a
  test or a check stop it happening again; a new rule in AGENTS.md is the last
  resort. Say in the PR which one you did.
- Keep going on work you were asked to do. Don't stop to ask again for what's
  already been asked for.
- Never commit what the repo keeps private: secrets, tokens, personal data, and
  anything listed under this file's "Private" section.
- `Closes #N` only when the issue is fully done; otherwise `Refs #N`.
- Tracked files never name a model. Which model does what is the machine's
  business, and model names go stale in weeks.

### Commits

Subjects are `area: plain sentence`, where the area is the module, directory or
tool the change is about: `autoland: merge only the head that was checked`.
Lowercase after the colon, no trailing period, no `feat:`/`fix:` prefixes. The
body, when there is one, says why.
<!-- house:rules end -->

## Find the relevant context

Start with `git status --short --branch` and the task's issue or PR, including
unresolved review feedback. Preserve unrelated work. Do not assume a previous
session's branch, milestone status, or verification still describes the head.

Read only the context needed for the task:

- Build, source ownership, or local development: `docs/development.md`.
- Next work and milestone status: `docs/roadmap.md`, then the linked GitHub issue.
- Product constraints: `PRINCIPLES.md`; boundaries: `docs/architecture.md`.
- Domain terms: `docs/domain-model.md`; record integrity: `docs/records-integrity.md`.
- Contribution lifecycle, verification, and refactor rules: `CONTRIBUTING.md`.
- Evidence-driven execution, graph records, authority, and effort policy:
  `docs/workflow.md`. Use it as the standing default for substantive work.
- Decisions and formats: the task index in `docs/development.md` links the
  relevant ADRs and specifications. Do not load the entire corpus by default.
- Preview deployment: `docs/preview.md`. Host configuration and deployed
  binaries are separate from this checkout.

The server is in `crates/consolebook-server/`: `src/` owns services and the thin
CLI, `migrations/` owns persisted constraints, and `tests/` owns integration
proof. `web/src/` owns the UI and typed API client; `web/e2e/` tests the binary
through a browser. `sessions.rs` means login sessions; `training_sessions.rs`
means training periods.

## Non-negotiable contracts

- Finalized records are immutable while retained. Corrections create successor
  versions or amendments. Lawful disposition is a separate authorized workflow.
- Agency variation is versioned configuration. Finalized records pin configuration
  and presentation snapshots; mutable reference data must not rewrite history.
- Operational dates are agency-local; duration and ordering use UTC instants.
- Capabilities and scope are enforced by domain services. HTTP and UI adapt
  those decisions. Authorization, integrity, retention, disposition, and exports
  require typed contracts and tests; heuristics cannot establish them.
- Startup verifies storage invariants and fails closed. `doctor` must diagnose
  without creating, migrating, or changing state (ADR 0003).
- Use invented agencies, people, incidents, identifiers, and narratives (see
  [Private](#private)). Security reports follow `SECURITY.md` privately.

## Work and verification

Non-trivial work requires one primary issue, an issue-linked branch, and a PR.
Search existing issues first; never push directly to `main`.

Stricter than the house planning rule: keep one canonical task graph in the
primary issue for substantive dependent work; simple changes need only a short
plan. Record ownership, dependencies, inputs, effort policy, state,
exact-revision evidence, and one next action. Verified results unlock
dependencies; failures trigger investigation and scoped repairs, with the
approach changed when evidence calls for it.

Stricter than the house model rule: guidance names roles, never models or
vendors (tool entrypoint file names excepted), and
`scripts/check-guidance-names.sh` enforces it. The **integrator** owns the
graph, scope, selection, review of every delegated result, integration, and
remote writes; an **implementer** makes a judgment-heavy change from a fixed
brief; a **reviewer** reads an exact candidate independently and edits nothing;
a **worker** does bounded work a command can accept; a **verifier** runs named
proof and produces receipts; a **scout** answers read-only questions of fact.
Route by how acceptance is decided, not by how hard the task looks. Record roles
in evidence, report an unavailable route instead of substituting, and never
claim an observed model the host does not show. Delegate only useful work with
explicit ownership, then verify the artifacts yourself.

Record the source of a task's authorization and any owner-set effort budget in
the issue; preserve that budget across children and resumes. Credentials and
graph edits grant no authority. There is no default repair-cycle cap. Without an
owner-set budget, continue in scope until acceptance or a concrete blocker;
revise the approach when failures repeat, preserve evidence, and finish
independent work before asking for a decision. See `docs/workflow.md`.

Build `web/` before Rust when UI or embedding matters. The command sequence and
browser prerequisites are in `CONTRIBUTING.md`. Required checks: `npm ci`,
`npm run check`, `npm run build` in `web/`; `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace`; then `cargo build -p consolebook-server` and
`npm run e2e` in `web/` with the documented Chromium prerequisite.
`cargo xtask proof` runs that sequence and leaves a receipt
(`.agents/skills/consolebook-proof/`).
Keep the pinned Rust toolchain in `rust-toolchain.toml`. `nix develop` gives the
toolchains and `nix flake check` runs the hermetic part of the gate (format,
clippy, Rust tests without embedded web assets, the proof runner's among
them, and the guidance guard); it does not replace the sequence above.

On top of the house reporting rule: give exact failures and investigate
intermittent ones. Design documents are not runtime proof; unit tests are not
recovery drills. Fix causes and in-scope defects; file exact-evidence issues for
separate work.

Large Rust modules follow `CONTRIBUTING.md`, which adds tiers to the house split
rule: over 1,000 lines requires an ownership-based reorganization, over 1,500
requires naming the boundary first, and over 2,500 requires a reviewed
decomposition path before major feature work unless urgent. Refactors state the
new owner, persisted/public impact, and focused proof.

Stricter than the house memory rule: agents extend the patterns they read, so
a workaround, a duplicate path, or a comment that justifies one is copied until
it is the pattern. Correct a recurring class at the strongest rung that applies:
(1) make it unrepresentable with types and one owning API; (2) enforce it with
the compiler, clippy, a test, or a guard; (3) write guidance; (4) rely on review
alone only when nothing stronger applies. Name the rung in the PR. When an
anti-pattern cannot be removed now, first add the guard that stops new
instances, then file the cleanup; gardening this way is ordinary work. Detail is
in [CONTRIBUTING.md](CONTRIBUTING.md#correct-the-class-not-the-instance).

Durable behavior changes require an ADR; changes to `PRINCIPLES.md` require one.
Use forward migrations, standard Rust formatting, `thiserror` for library
errors as they emerge, `anyhow` at application boundaries, and no `unsafe`.
Logs exclude sensitive
content; existing first-run setup-code output is the documented exception
(ADR 0004), not permission for additional secret logging.

This file owns the concise shared contract; `CONTRIBUTING.md` owns contribution
rules and gates, `docs/workflow.md` owns execution procedure,
`docs/development.md` owns the source map, and ADRs own product decisions. Tool-specific
entrypoints (`CLAUDE.md`, `.agents/rules/`, `.github/copilot-instructions.md`)
only point here. Keep machine preferences — including which model fills which
role — and session handoffs out of this file; they belong in ignored local files.

<!-- house:prs v1 begin. Copied from FieldmouseWorks/burrow house/prs.md, verbatim. Change it there, not here: burrow's house sync rewrites everything between these markers. -->
## Pull requests

Open PRs ready for review, never as drafts, and leave them to the steward: it
asks the review bot for a review, and once the bot finds nothing on your head,
every review thread is resolved and CI is green, it merges the PR. Don't merge
it yourself. Fix or answer every finding: push the fix and reply with the
commit, or reply with why it's wrong, then resolve the thread. A PR touching a
one-way path (secrets, migrations, CODEOWNERS) gets a judgment first: it lands
unless it can't be undone or is the owner's to decide.
<!-- house:prs end -->

A PR touching an ADR or `PRINCIPLES.md` is settled the same way and then waits
for the owner's merge.

## Code Review Rules

For reviewers, the review bot included. Flag as P1:

- a change that makes a check pass without fixing the thing it checks
- tests or checks weakened, skipped or deleted to get green
- secrets, tokens or personal data in the diff or the logs it adds
- behavior changed beyond what the PR says it does
- real agencies, people, incidents or operational records in fixtures, docs or
  logs (only invented ones belong here)
- a migration that edits an existing one instead of moving forward, or changes
  persisted constraints without saying so
- a durable behavior change with no ADR, or a `PRINCIPLES.md` change without one
- `unsafe`, or new sensitive content in logs
- an action outside the allowlist in CONTRIBUTING.md, or one not pinned to a
  full commit SHA
- release order broken: tags and published releases are immutable, so the
  draft-upload-then-publish sequence is load-bearing
- new behavior added to a Rust module over the size limits in CONTRIBUTING.md
  without the reorganization they require
- a finalized record changed or deleted in place instead of through a
  successor version or amendment, or a finalized record that no longer pins
  its configuration and presentation snapshots
- capability or scope decided in an HTTP handler, the UI or a heuristic
  instead of the domain service that owns it
- startup storage checks that fail open, or `doctor` creating, migrating or
  changing state
- operational dates taken from UTC instead of the agency's local date, or
  durations and ordering computed from local times

## Private

Never committed, logged, or put in issues (stricter than the house rule: tests
and logs included):

- Real records, operational material, and anything from a real agency,
  person, or incident. Fixtures, examples, and seed data are invented.
- Personal data and credentials, including first-run setup codes beyond the
  documented ADR 0004 output.
- Security reports, which go through `SECURITY.md` privately.
- Machine-local agent preferences and session handoffs, kept in the files
  `.gitignore` lists for them.
