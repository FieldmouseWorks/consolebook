---
name: consolebook-proof
description: Route a consolebook change to its smallest focused proof, then run the full CONTRIBUTING.md gate sequence with one command that stops at the first failure and leaves a local receipt. Use before claiming any change is verified, before freezing a PR candidate, and when asked for proof or a receipt.
---

# Consolebook proof

This skill executes proof; it owns no policy. `CONTRIBUTING.md` owns the gate
sequence and prerequisites, `docs/workflow.md` owns what a receipt must say and
how local, hosted, and merged-main results stay distinct, and
`docs/development.md` owns the map from area to code and tests.

## 1. Iterate with the smallest proof

Focused checks shorten the loop. They never replace the full gates, and their
output is not a gate receipt. Pick by what the change touches:

| Area | Focused proof while iterating |
| --- | --- |
| Domain service or HTTP adapter in `crates/consolebook-server/src/` | The capability-named integration test from the `docs/development.md` task row: `cargo test -p consolebook-server --test <capability>`; unit tests with `--lib` |
| Migrations, constraints, triggers | The integration tests for the persisted contract, plus fresh and upgraded paths where a test owns them (for example `--test enrollment_event_schema`), and `--test doctor_read_only` when diagnostics read the schema |
| Web UI or typed API client in `web/src/` | `(cd web && npm run check)`; for a workflow, build web then `cargo build -p consolebook-server` and run the one spec: `(cd web && npx playwright test e2e/<spec>.spec.ts)` |
| Browser harness in `web/e2e/` | `(cd web && npx playwright test e2e/server.spec.ts)` and the specs that use the changed fixture |
| Guidance and documentation | `scripts/check-guidance-names.sh`, `git diff --check`, and the link and command checks in `CONTRIBUTING.md` (Documentation and fixtures) |
| This skill or `crates/xtask` | `cargo test -p xtask`, then `cargo xtask proof --list` to see the sequence it will run |

No area is exempt from the full gates, documentation included.

## 2. Freeze the candidate, then run every gate

Review the diff first. Then, from anywhere in the repository:

```sh
cargo xtask proof                     # Playwright-managed Chromium, as CONTRIBUTING.md's sequence installs it
cargo xtask proof --chromium PATH     # a system Chromium, through CONTRIBUTING.md's documented alternative
```

`CONSOLEBOOK_E2E_CHROMIUM` in the environment counts as `--chromium`. The
runner reads the sequence from the first `sh` block under "Build and
verification" in `CONTRIBUTING.md` and runs each line in order from the
repository root, so the skill and that file cannot disagree; if the block's
shape changes in a way the runner does not recognize, it refuses to run
(exit 2). It prints each step, its exit status and duration, and the browser
the suite will use; it stops at the first failing step.

Run as an unprivileged user: the Unix permission tests assume one, and the
receipt records whether the run could bypass file permissions. Where only
root is available (a disposable container, for example), drop the Linux
capabilities that bypass them instead; the receipt then records the run as
unprivileged, read from the effective capability set:

```sh
setpriv --inh-caps=-dac_override,-dac_read_search,-fowner \
  --bounding-set=-dac_override,-dac_read_search,-fowner cargo xtask proof
```

## 3. Read and report the receipt

Every run, passed, failed, or interrupted, writes a new directory under the
ignored `.proof/` and never deletes one. It holds:

- `receipt.json`: candidate SHA, tree, and dirty state (status, tracked diff
  hash, untracked file hashes), whether the candidate changed during the run,
  the invocation and working directory, start, finish, and duration, the
  `CONTRIBUTING.md` hash, host and privilege, toolchain versions, the browser
  kind, path, and version, each step's command, status, exit, duration, log,
  and log SHA-256, the server binary's SHA-256, the web build manifest hash,
  and the browser suite's own counts (expected, unexpected, flaky, skipped);
- `receipt.md`: the same receipt as a paste-ready summary;
- `NN.log` per step that ran, `web-build.sha256`, the asset manifest, and
  `e2e-report.json`, Playwright's report. The browser step fails without that
  report or with zero tests, whatever its exit status, so a gate line that
  never reaches the suite cannot pass.

The integrator posts `receipt.md` to the primary issue or PR, labeled as a
local result, and links the hosted `pr-gate` run separately once it is
observed. The receipt does not prove a hosted result, a recovery drill, or a
merged-main revision.

A receipt is stale when the candidate changes after it: rerun and keep both.
If the runner warns that the candidate changed during the run, that receipt
is stale already. A failed receipt stays as evidence; read its step log,
identify the cause, and follow the failure loop in `docs/workflow.md` instead
of rerunning unchanged.
