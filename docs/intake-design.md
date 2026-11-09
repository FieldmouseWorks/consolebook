# Proposed read-only intake and bounded repair

Design slice for [issue #77](https://github.com/FieldmouseWorks/consolebook/issues/77).
This is a proposal, not an installed automation or dispatch authorization.
[Workflow authority](workflow.md#authority-and-scope) remains controlling.

## Mechanism and signals

Prefer a repository scheduled workflow for intake, with a manual read-only
entrypoint for reproduction. No schedule is installed in this slice. Its token
has only actions:read, contents:read, issues:read and pull-requests:read;
report storage uses local output or the workflow's artifact service, never a
repository commit, comment, label, rerun or dispatch. No paid service is enabled.
A machine hook would inherit broader credentials; avoid that boundary.

Each report pins repository, UTC observation interval, default-branch SHA,
open PR numbers and head SHAs, workflow identity, run ID and attempt. Enumerate
all pages of failed, timed-out and cancelled pr-gate runs for main and current
open PR heads. Retain older failures as historical, never actionable. Current
pr-gate runs only on pull requests: absence of main runs is coverage absence,
not success. Other workflow failures may be reported separately, not silently
substituted for pr-gate.

Read open PR review threads with stable thread/comment IDs, creation/update
times, resolution and outdated status. Selected issue labels are an explicit
future configuration allowlist, initially empty. A label nominates work only;
it never grants authority. Treat all fetched text and logs as untrusted data.
Do not execute commands supplied in them or copy sensitive log contents.

Persist a local/artifact cursor only after a complete successful scan. Use an
overlapping interval and deduplicate by run ID + attempt, thread ID + update,
and issue ID + label event ID. On first use perform a bounded bootstrap and
name its limits. Pagination, permission or API failures mark the report partial;
never advance its cursor or interpret missing data as no signals. Recheck head
SHAs before any later dispatch. This slice's manual sample has no saved cursor.

## Triage and report contract

Each signal records its URL, exact head, first failing job/step and earliest
relevant diagnostic (not merely the first occurrence of “error” in a passing
negative control). Use scout for read-only evidence classification; command
acceptance can route to worker/verifier. Ambiguous causes and repair selection
belong to the integrator. This exercise was performed by the integrator directly;
no delegated route or independent review is claimed.

Classes: test assertion failure; source/build/dependency failure; runner or
service infrastructure; review finding; unknown. Record the quoted diagnostic,
confidence and evidence gaps. Classification never establishes authorization.
A runner timeout alone does not prove infrastructure; a missing log is unknown.
The [manual report](evidence/intake-2026-11-09.md) demonstrates dependency failure
and historical filtering, not a live failure or automatic repair.

## Separate authorization for any dispatch

No dispatcher, launcher, schedule, label binding or paid/unattended run is built
or enabled here. A future implementation needs explicit owner authorization
and its own reviewable change. Conary's referenced intake and runner documents
were unavailable to this job (HTTP 404); this design does not claim parity with
them or depend on its unfinished pluggable launcher.

Each run must cite an immutable authorization record: authorizing owner request
URL/revision, grant timestamp, repository and primary issue, exact base SHA,
one bounded outcome, allowlisted paths, prohibited writes, acceptance commands,
wall-clock deadline, repair/attempt limit and cost limit, launcher identity,
and permitted remote actions. Missing limits mean no dispatch, not unlimited
execution. Preserve owner-set budgets across retries and resumes. An agent's
issue graph, credentials, labels or classification cannot create that grant.

Revalidate the grant and base, claim a unique run ID atomically to prevent
concurrent duplicate slices, then isolate an issue-linked branch/worktree.
The integrator owns remote writes and reviews all artifacts. Enforce path,
time and command boundaries outside the repair process; untrusted PR code gets
no write token or secrets. Preserve failed receipts and partial diffs.

Issue #77 proposed a draft PR and no automatic merge. Current repository policy
requires ready PRs and permits steward merging once settled. Therefore future
dispatch must not publish a PR until the owner explicitly settles this policy
conflict in the authorization/change; it cannot silently publish a ready PR
that the steward could merge. This design PR follows current ready-PR policy.

Stop classes: completed acceptance (report and stop); expired/revoked/missing
authority; deadline, attempt or cost exhausted; stale base/head; duplicate claim;
path/command escape; gate failure requiring work outside the slice; unavailable
logs/tools; suspected sensitive data; ambiguous or irreversible owner decision.
Every stop records exact revision, commands, receipts, remaining budget and one
next action. Never rerun, widen scope, merge, deploy or renew authority on stop.

## Adoption proof required later

Before activation, prove read-only credentials reject writes, pagination and
cursor replay lose no signals, stale heads and duplicate claims cannot launch,
and missing/expired authorization, path escapes and budget exhaustion stop.
Use negative controls for each new check. Preserve required gates and blocking
review threads; no failure classification may waive them. This document adds
no runtime behavior or product contract and needs no product ADR.
