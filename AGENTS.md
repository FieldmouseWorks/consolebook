# Repository Guidelines

Consolebook is pre-alpha training-record software for emergency communications
centers: one Rust executable, one SQLite data directory, and a static SvelteKit
UI. No required external services, telemetry, or production Node.js runtime.

<!-- house:rules v1 begin. Copied from FieldmouseWorks/burrow house/rules.md, verbatim. Change it there, not here: burrow's house sync rewrites everything between these markers. -->
## House rules

These hold in every repo. This file's own sections add to them and win where
they're more specific.

- Design the class, not the instance. When a bug, finding or stall shares a
  cause with others, fix the cause once, where it lives (one owning path, a
  type, a check), so none of its cases come back.
- Do work once. Look for what's already open or built first, extend the
  shared crate, client or job that does it rather than write another copy,
  and never run work twice that once would do.
- Fix the whole cause: no workarounds, shims, fallbacks or half-migrations.
  When you replace something, delete the old path in the same change.
- One fact, one home. Each rule or fact lives in one place; link to it
  instead of restating it.
- Pipeline options live in the settings source: change them through its
  door, read them with `proj settings show`, and link the
  [settings reference](https://github.com/FieldmouseWorks/burrow/blob/main/docs/settings-reference.md)
  rather than restate a value or where one is kept.
- Before you edit, say what done looks like and how it will be checked.
- Report only the checks you ran and what they said. Skipped, unavailable
  or "should pass" is never a pass. Keep local and CI results apart.
- A new check comes with proof that it fails on the bug it's for.
- The code is the memory. When something goes wrong, make the code, a type,
  a test or a check stop it happening again; a new rule here is the last
  resort. Say in the PR which one you did.
- Keep going on work you were asked to do: don't ask again for what's
  already been asked for.
- Never commit what the repo keeps private: secrets, tokens, personal data,
  and anything under this file's "Private" section.
- `Closes #N` only when the issue is fully done; otherwise `Refs #N`.

CI's house check holds every PR to its title's style and to the rules below,
each of which a PR can pass with a line in its body starting as shown and a
reason of four words or more. Editing the body or title runs it again.

- Rust by default, shell included: new code meant to last starts in Rust
  unless its platform picks the language, and a coin toss goes to Rust. A PR
  that adds Python, or grows Python and shell past a fix, says `Why not
  Rust:` (a file a tool generates, glue that only runs another program, a
  throwaway script, a tool still being tuned). Port existing Python and
  shell whenever Rust buys anything, system and pipeline code first.
- Work starts from an event (a push, a webhook, a unit finishing), not a
  clock. A PR that adds a timer or a cron schedule says `Why a timer:`.
- Split a code file before it passes 1,000 lines, or say `Why not split:`.
- AGENTS.md is a map: what the repo is, its own rules, and where things live
  (a feature map, the doc that owns each subject). Detail goes in the doc
  that owns it, linked from here. A PR that grows the repo's own part (the
  lines outside these blocks) past 100 lines says `Why a longer guide:`.
- Tracked files never name a model: which model does what is the machine's
  business, and names go stale in weeks. A line that has to (a recorded
  screen in a test) says `Why a model name:`.

### Commits

Subjects are `area: plain sentence`, the area being the module, directory or
tool the change is about (`autoland: merge only the head that was checked`):
lowercase after the colon, no trailing period, no `feat:`/`fix:`. The body,
if any, says why. A PR's title is the subject main gets, so it's written the
same way. Keep the git identity your machine gives you: a commit signed with
a key that isn't its author's shows as Unverified, and CI fails it.
<!-- house:rules end -->

## Where things live

Start with `git status --short --branch` and the task's issue or PR, with its
unresolved review feedback; trust no earlier session's word about the head.
Then read only what the task needs:

- Source map, build and local development: [docs/development.md](docs/development.md),
  whose task index links the ADRs and specifications for each area.
- Execution: task graphs, roles, authority and effort policy:
  [docs/workflow.md](docs/workflow.md), the default for substantive work.
- Next work and milestones: [docs/roadmap.md](docs/roadmap.md), then its issue.
- Product constraints: [PRINCIPLES.md](PRINCIPLES.md); boundaries, terms and
  record integrity: [docs/architecture.md](docs/architecture.md),
  [domain-model.md](docs/domain-model.md), [records-integrity.md](docs/records-integrity.md).

The server is `crates/consolebook-server/` (`src/` services and the thin CLI,
`migrations/` persisted constraints, `tests/` integration proof); the UI and
its typed API client are `web/src/`, and `web/e2e/` drives the binary in a
browser. `sessions.rs` is login sessions; `training_sessions.rs` is training
periods.

## Non-negotiable contracts

The record contracts (finalized records immutable, configuration versioned and
pinned, dates agency-local, authorization owned by domain services, startup
failing closed, migrations forward-only, an ADR for durable behavior) are the
P1 list under [Code Review Rules](#code-review-rules); [PRINCIPLES.md](PRINCIPLES.md)
and the ADRs say why. Use invented agencies, people and incidents only
([Private](#private)); security reports follow `SECURITY.md` privately.

## Work and verification

Non-trivial work has one primary issue (search first), an issue-linked branch
and a PR; never push to `main`. Substantive dependent work keeps one task
graph in that issue, and guidance names roles, never models or vendors
(`scripts/check-guidance-names.sh` enforces it): both are stricter than the
house rules, and [docs/workflow.md](docs/workflow.md) owns them.

The gate is `cargo xtask proof`, which runs the web and Rust sequence in
[CONTRIBUTING.md](CONTRIBUTING.md) and leaves a receipt; `nix flake check` runs
only its hermetic part. CONTRIBUTING.md also owns contribution rules, the
size tiers for Rust modules and the correction ladder, whose rung a PR names.
Machine preferences and session handoffs stay in ignored local files;
`CLAUDE.md`, `.agents/rules/` and `.github/copilot-instructions.md` only point
here.

<!-- house:prs v1 begin. Copied from FieldmouseWorks/burrow house/prs.md, verbatim. Change it there, not here: burrow's house sync rewrites everything between these markers. -->
## Pull requests

One task, one PR: finish what the task set out to do, then open one PR, so
it goes through review and CI once. No PR per step or round, and no stacks.
A fix to the pipeline (the steward, CI, the machine) lands as soon as it's
ready, and so does a change to these rules that the work needs, in a PR of
its own: reviewers read the default branch's rules, not the PR's.

Open PRs ready for review, never as drafts, and leave them to the steward:
once the review bot finds nothing on your head, every thread is resolved and
CI is green, it merges. Don't merge it yourself. Answer every finding: push
the fix and reply with the commit, or reply with why it's wrong, then
resolve the thread. A PR touching a one-way path (secrets, CODEOWNERS) gets
a judgment first, and lands unless it can't be undone or is the owner's
call.

A red default branch is the steward's to fix, from a branch named
`autoland/red-` and the commit. Don't fix it yourself, in any repo, unless
the pipeline's stuck issue says the steward's jobs gave up; then fix it from
a branch of that name.
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
