# Evidence-driven workflow

This page sets a reviewable procedure for bounded project work. It is guidance
for people and agents, not a scheduler, dispatcher, or unattended restart.

## Sources of truth

- [AGENTS.md](../AGENTS.md) is the concise shared contract and routes readers
  here for the detailed workflow.
- [CONTRIBUTING.md](../CONTRIBUTING.md) owns the issue, branch, PR, engineering,
  and mandatory verification requirements. Follow its current commands and
  prerequisites; this page does not replace or waive them.
- [The roadmap](roadmap.md) owns current product and milestone state. It is a
  status summary, not an authorized queue of work.
- The primary GitHub issue owns the task graph and acceptance criteria. For
  this setup, [issue #71](https://github.com/FieldmouseWorks/consolebook/issues/71)
  is canonical. Comments may preserve dated progress and receipts; the PR
  links the issue and describes the change without a competing live graph.
- The current PR and review threads own review feedback and hosted results.
  The [PR gate](../.github/workflows/pr-gate.yml) defines current hosted web,
  Rust, and browser checks.

## Authority and scope

The user's current request is the source of authorization; repository rules
define how authorized work is done. Record the granted scope and its source in
the primary issue. An issue or graph written by an agent records authority; it
does not grant it. Explicit authorization carries across sessions until it is
revoked or the task leaves scope. Credentials prove access, not permission.
This document grants no authority.

For setup issue #71, the owner's request authorizes scoped local edits and
checks, an issue-linked branch and PR, issue/PR evidence updates, isolated
setup work, and cleanup of owned processes and scratch resources; the issue
records that grant. No merge, deployment, non-PR publication, paid-service, or unrelated-cleanup
authority was granted. Later tasks must use their current authorization.

For an authorized merge, verify the exact merged main revision, archive and
read back the issue evidence, then clean up owned branches, worktrees, and
processes. Deploy only when explicitly authorized. Without merge authority,
finish the reviewable work and leave one concrete decision. This setup ends at
a PR ready for the maintainer; preserve owned work until approval arrives.

Use invented agencies, people, incidents, identifiers, and narratives. Keep
operational records, personal data, and credentials out of issues, commits,
fixtures, and logs. Follow [SECURITY.md](../SECURITY.md) for security reports.

## Plans and task graphs

For simple, low-risk changes, record the scope, source, acceptance check, and
next action in the issue or PR. Use a graph for dependent steps, parallel
owners, or evidence checkpoints. The primary issue owns it; a graph records
intended work, not proof.

Give each node a stable ID and these fields:

```text
ID:
Outcome: observable result
Depends on: IDs, or —
Owner / model: named owner; requested route and observed model at max, if exposed
Inputs: issue, files, revision, and other source records
Acceptance: observable condition and check
State: pending | ready | active | blocked | verified
Evidence: exact revision, receipt, review, or —
Effort: owner-set budget and source, if any; carry across children/resumptions

Next action: one concrete action for the whole graph
```

Keep the graph compact, link shared inputs once, and update state only from
observed results. Maintain one next action in the issue. Comments may append
dated receipts; preserve failures. Before another outcome, archive and read
back the completed graph snapshot.

Graph edits record progress; they do not change acceptance, authority, or an
owner-set budget. Missing evidence is a specific blocker, not a completed check.

## Model routes and ownership

Use the model route assigned to the task and record the requested model and
reasoning level accurately. The project routes work as follows:

| Work | Route |
| --- | --- |
| Overall planning, architecture, difficult tradeoffs, review, and integration | GPT-6 Astra (`gpt-6-astra`) / max |
| Complex implementation, substantial refactoring, or difficult debugging | GPT-6 Sol (`gpt-6-sol`) / max |
| Bounded exploration, documentation, routine checks, and mechanical edits | GPT-6 Luna (`gpt-6-luna`) / max |

The primary owner maintains the graph, scope, and acceptance; reviews
contributions and independently verifies results. Astra selects only ready
nodes whose dependencies are verified. Delegated owners get a narrow outcome,
explicit file or responsibility ownership, inputs, acceptance, and any
owner-set budget. Preserve others' changes. Run work concurrently only
with independent inputs and ownership; inspect every result, including failures.

Record requested routing separately from runtime evidence. A configuration
setting or model catalog entry does not prove which model handled a turn. Do
not silently substitute a route or describe a requested route as an observed
invocation. If the host cannot expose the actual route, say that precisely.

DeepSeek remains paused. Use it only if the owner explicitly re-enables it,
under the current supervised-use instructions. Record substantive results
and independent checks in the issue or existing evaluation record. Editing an
instruction file does not change an already running conversation's model.

## Effort and continuation

Honor any explicit effort budget set by the task owner and record its source.
It persists across delegated children and resumptions; changing the graph,
branch, or session does not reset it. If an explicit budget is exhausted,
honor it and report the blocker; an extension requires the owner's decision.

There is no default repair-cycle cap. Without an owner-set budget, continue
scoped investigation until acceptance is met or a concrete blocker prevents
it. Repeated failures are evidence: revisit the hypothesis or method when results
undermine the current approach. Preserve each failure and repair receipt; do
not repeat speculative actions unchanged. At a blocker, record what is missing,
finish independent preparation, and ask only for the decision needed to proceed.

## Execution and evidence loop

1. **Observe the current state.** Check the branch, working tree, current
   revision, primary issue, open PRs, and unresolved review comments. Confirm
   which source files and results still describe the current checkout. Read
   only the project context needed for the task, starting with the routes in
   AGENTS.md and the task's linked records.
2. **Set the boundary.** State the intended outcome, exclusions, authorized
   actions, source records, acceptance checks, and any owner-set effort budget.
   For nontrivial work, use one primary issue, an issue-linked branch, and a PR
   as required by CONTRIBUTING.md. Search existing issues before creating one.
3. **Plan and assign.** Use a short plan for simple work or the issue graph
   template above for dependent work. Give each child clear ownership and
   acceptance. Keep one shared next action. Do not start roadmap work merely
   because it appears next in sequence.
4. **Make one bounded change at a time.** Keep edits within the accepted
   outcome. Check the cheapest relevant existing proof first. Any new
   correctness check needs a meaningful negative control that rejects a known
   wrong behavior; do not restate the implementation as its own test.
5. **React to observed failures.** Read the output and inspect all settled
   parallel work. Identify a cause before repairing; when a failure repeats,
   change the hypothesis or method based on what the evidence shows. Do not
   repeat unchanged speculative retries. Continue in scope toward acceptance.
   If blocked by missing evidence, unavailable authority, or an owner decision,
   record the precise blocker and finish independent preparation. Install a
   missing local tool when authorized; otherwise record that limitation.
6. **Freeze the candidate.** Review the diff, then record the reviewed
   candidate's SHA, tree state, relevant dirty diff, and gate inputs before
   expensive checks. Run checks against that candidate. An edit affecting a
   gate's inputs makes its receipt stale; rerun the affected check on the new
   candidate and preserve both receipts.
7. **Run and distinguish gates.** Follow the complete current sequence and
   prerequisites in CONTRIBUTING.md. Focused checks support iteration; they
   do not replace required gates. Record local results separately from hosted
   PR results and from checks on a merged main revision. Distinguish the
   default browser, a system browser, a downloaded Playwright browser, and a
   hosted runner when that affects the result. A documentation-only change has
   no automatic exemption from the repository's required gates.
8. **Review the evidence.** Inspect the final diff and relevant raw logs.
   Review behavior against acceptance, not just the success summary. Label
   self-review, independent model review, human review, and automated checks
   accurately; one is not evidence that another happened. Fix in-scope causes
   and record exact evidence for separate issues.
9. **Publish and close the graph.** When authorized, publish the PR and link it
   from the primary issue. Record hosted results after observing them. Archive
   and read back the completed graph and evidence before declaring the task
   complete. Stop only processes and scratch resources owned by the task.
   Leave unrelated local work intact.

For parallel local and hosted checks, first freeze the same candidate and
ensure builds, browser profiles, databases, ports, and output paths cannot
collide. Parallel checks may then proceed when the PR has been authorized and
published. If a check fails, preserve its receipt and follow the evidence-driven
failure loop; a later green run does not erase an earlier failure.

## Receipts and resuming

Every material check or review receipt names:

- actual timestamp and duration;
- exact command or review action and working directory;
- candidate revision and relevant inputs, including browser or runner where
  they affect the result;
- exit status or observed finding; and
- log, artifact, or review location and SHA-256 hashes generated from material
  local artifacts: logs, the exercised binary, and an asset manifest when
  applicable. Output hashes are separate from source/tree hashes; record a
  precise limitation when hosted artifacts cannot be inspected.

Preserve failed receipts and exact failures. Do not invent elapsed time,
runtime model, reviewer identity, check results, or a green status. Keep
sensitive values out of evidence. Link a hosted run directly instead of
copying its summary as if it were a local check. Claim a speedup only with
comparable completed measurements for the same scope and inputs.

On resume, inspect the current issue graph, branch and revision, owned
processes, working tree, and freshness of each receipt before continuing.
Reuse evidence only when its inputs still match. Reconcile the graph with
observed results, carry forward any owner-set budget and its usage without
inventing a counter, and state one next action. Stale evidence or an agent's
completion claim alone does not satisfy acceptance.

## Instruction loading and workflow changes

Codex selects global `AGENTS.override.md` when present, otherwise global
`AGENTS.md`, then reads from the project root toward the current directory. It
loads at most one file per directory, preferring `AGENTS.override.md` over
`AGENTS.md`; check nested instructions. Generic Markdown is not an entrypoint:
active project instructions must link here or the user must request this
page. See the official
[AGENTS.md configuration guide](https://learn.chatgpt.com/docs/agent-configuration/agents-md).

When an acceptance criterion requires checking effective instructions in a
fresh session, perform that probe in a genuinely fresh Codex session or record
the precise host limitation. Do not claim that editing instructions changed
the already running session.

Improve this procedure only from a specific observed problem. Record the
problem and proposed scope in the existing issue, make a small reviewable
change, and check its links and instructions. Preserve the current issue/PR
lifecycle, authority boundary, continuation policy, and required gates. This
document does not create new CI, a linter, a workflow framework, or an approval
system.
