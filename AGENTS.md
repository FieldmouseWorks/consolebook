# Repository Guidelines

Consolebook is pre-alpha training-record software for emergency communications
centers: one Rust executable, one SQLite data directory, and a static SvelteKit
UI. No required external services, telemetry, or production Node.js runtime.

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
- Use invented agencies, people, incidents, identifiers, and narratives. Real
  records, operational material, personal data, and credentials never enter the
  repository, tests, logs, or issues. Security reports follow `SECURITY.md` privately.

## Work and verification

Non-trivial work requires one primary issue, an issue-linked branch, and a PR.
Search existing issues first; never push directly to `main`. `Closes #...` means
all acceptance criteria are satisfied; otherwise use `Refs #...`.

Define an observable outcome and acceptance check before editing. Keep one
canonical task graph in the primary issue for substantive dependent work;
simple changes need only a short plan. Record ownership, dependencies, inputs,
effort policy, state, exact-revision evidence, and one next action. Verified
results unlock dependencies; failures trigger investigation and scoped repairs,
with the approach changed when evidence calls for it.

Guidance names roles, never models or vendors (tool entrypoint file names
excepted): the **integrator** owns the graph, scope, selection, review of every
delegated result, integration, and remote writes; an **implementer** makes a
judgment-heavy change from a fixed brief; a **reviewer** reads an exact candidate
independently and edits nothing; a **worker** does bounded work a command can
accept; a **verifier** runs named proof and produces receipts; a **scout** answers
read-only questions of fact. Route by how acceptance is decided, not by how hard
the task looks. Which model or effort fills a role is each contributor's
machine-local preference and stays out of tracked files; record roles in
evidence, report an unavailable route instead of substituting, and never claim
an observed model the host does not show. Delegate only useful work with
explicit ownership, then verify the artifacts yourself.

Carry task authorization forward without asking again. Record its source and
any owner-set effort budget in the issue; preserve that budget across children
and resumes. Credentials and graph edits grant no authority. There is no
default repair-cycle cap. Without an owner-set budget, continue in scope until
acceptance or a concrete blocker; revise the approach when failures repeat,
preserve evidence, and finish independent work before asking for a decision.
See `docs/workflow.md`.

Build `web/` before Rust when UI or embedding matters. The command sequence and
browser prerequisites are in `CONTRIBUTING.md`. Required checks: `npm ci`,
`npm run check`, `npm run build` in `web/`; `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace`; then `cargo build -p consolebook-server` and
`npm run e2e` in `web/` with the documented Chromium prerequisite.
Keep the pinned Rust toolchain in `rust-toolchain.toml`.

Report only verification actually run, with exact failures. Investigate
intermittent failures. Design documents are not runtime proof; unit tests are
not recovery drills. Fix causes and in-scope defects; file exact-evidence
issues for separate work.

Follow `CONTRIBUTING.md` before adding behavior to large Rust modules: over
1,000 lines requires an ownership-based reorganization, over 1,500 requires
naming the boundary first, and over 2,500 requires a reviewed decomposition
path before major feature work unless urgent. Refactors state the new owner,
persisted/public impact, and focused proof.

**The codebase is memory.** Agents extend the patterns they read, so a
workaround, a duplicate path, or a comment that justifies one is copied until
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
Use short imperative Conventional Commit subjects. Logs exclude sensitive
content; existing first-run setup-code output is the documented exception
(ADR 0004), not permission for additional secret logging.

This file owns the concise shared contract; `CONTRIBUTING.md` owns contribution
rules and gates, `docs/workflow.md` owns execution procedure,
`docs/development.md` owns the source map, and ADRs own product decisions. Tool-specific
entrypoints (`CLAUDE.md`, `.agents/rules/`, `.github/copilot-instructions.md`)
only point here. Keep machine preferences — including which model fills which
role — and session handoffs out of this file; they belong in ignored local files.
