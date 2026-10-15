# Roadmap

The roadmap is ordered by risk. Dates come later; fake schedules are how software projects begin lying to themselves.

## Current state and next gates

Release baseline: **0.3.0** (2026-10-15, tag `v0.3.0`; see the
[changelog](../CHANGELOG.md)). 0.2.0 (2026-10-15) added safer upgrades and
recovery for a pilot, and 0.1.0 (2026-10-14) was the first versioned release. Milestones 0–4 are complete, including immutable
finalized records, acknowledgments, amendments, summaries, and task signoffs
([Milestone 4 history](https://github.com/FieldmouseWorks/consolebook/issues/32)).
Milestone 5 has file-verifiable [record exports](https://github.com/FieldmouseWorks/consolebook/pull/46)
and [trainee packets](https://github.com/FieldmouseWorks/consolebook/pull/50),
[retention policies and holds](https://github.com/FieldmouseWorks/consolebook/pull/66),
[trainee signoff history](https://github.com/FieldmouseWorks/consolebook/pull/68),
and a [complete installation export](formats/installation-export.md)
([ADR 0028](decisions/0028-installation-export.md)) that carries everything but
credentials.

Operability as of 0.3.0:
- versioned releases with a traceable build identity
  ([ADR 0022](decisions/0022-product-versioning.md)), installed by the installer
  published with each release, with build provenance
  ([ADR 0026](decisions/0026-release-distribution-and-provenance.md));
- a one-command Linux install and upgrade
  ([ADR 0023](decisions/0023-linux-installation-lifecycle.md),
  [install guide](install.md)), where an upgrade that migrates takes a
  validated backup first;
- HTTPS behind a reverse proxy with `Secure` session cookies
  ([ADR 0025](decisions/0025-public-url-and-secure-cookies.md));
- verified backup copies off the server that a replacement server cannot
  bury, checked by `doctor` ([ADR 0024](decisions/0024-off-server-backup-copies.md),
  [ADR 0027](decisions/0027-backup-copy-encryption.md));
- `doctor` reports exactly what `serve` would refuse, and only warns about
  what the service does on its next start
  ([ADR 0029](decisions/0029-doctor-migration-verdict.md));
- a recorded [restore drill](restore-drill.md) and an
  [operations guide](operations.md).

`scripts/proof.py` runs the full local gate sequence and leaves a receipt.

Material limits: confirmed disposition, tombstones, portable policy-boundary
evidence, and deterministic PDFs remain Milestone 5 work. The restore drill has run
scripted on one machine, not yet on real hosts with a network share, and does
not yet run on every release ([#119](https://github.com/FieldmouseWorks/consolebook/issues/119)).
Pre-alpha status still applies.

Work follows the [evidence-driven workflow](workflow.md): one primary issue
owns each outcome's task graph, permission and effort record, next action,
and verification receipts.

Next product gate: finish the approved retention/disposition design in
[#64](https://github.com/FieldmouseWorks/consolebook/issues/64) under
[#44](https://github.com/FieldmouseWorks/consolebook/issues/44). This is sequencing,
not authorization to begin another outcome. Reconcile live issues and PR reviews
before selecting work, including the recorded intermittent browser test
failure [#83](https://github.com/FieldmouseWorks/consolebook/issues/83).

For continuation, check the
[open issues](https://github.com/FieldmouseWorks/consolebook/issues?q=is%3Aissue+is%3Aopen)
and current PR reviews. The [2026-09-05 audit](audits/2026-09-05.md) records
verification, known gaps, and recommended sequencing at that revision; it is
not a substitute for live issue state.

## Milestone 0 — Foundation

- establish principles and architecture decisions;
- define public domain vocabulary;
- create a buildable Rust workspace;
- publish the AGPL-3.0-only license and contribution terms;
- define contribution and security-reporting paths; and
- specify canonical record bytes.

**Exit:** the project can explain what it is building and what it refuses to become.

## Milestone 1 — Operable shell

- command-line configuration;
- first-run setup code;
- local administrator creation;
- administrator-issued password reset and sole-administrator local recovery;
- embedded SvelteKit shell for setup, sign-in, recovery, status, and in-app notices;
- SQLite migrations and explicit connection invariants;
- health and `doctor` commands;
- structured logging without sensitive record content; and
- automatic validated backups with a tested restore path.

**Exit:** an empty installation can initialize, restart, back up, restore, and diagnose itself.

## Milestone 2 — Versioned program configuration

- programs and immutable program versions;
- phases and non-linear transitions;
- competencies, tasks, forms, and rating scales;
- publishing and enrollment pinning; and
- configuration export/import; and
- web interface for authoring, comparing, publishing, and enrolling program versions.

**Exit:** a complete invented training program can be published, enrolled, exported, and reproduced.

## Milestone 3 — Sessions and drafts

- users, capabilities, and assignment-scoped access;
- enrollments and phase history;
- training sessions with explicit time semantics;
- daily evaluation drafts;
- contributor and ownership-transfer history;
- manual and automatic draft persistence; and
- review/change-request workflow;
- persisted in-app workflow notices; and
- trainer and coordinator web interface for sessions, drafts, assignments, and review.

**Exit:** trainers can document an invented session collaboratively without losing attribution.

## Milestone 4 — Defensible records

- canonical record format;
- immutable finalized versions;
- rating and narrative rules;
- weekly summaries linked to daily versions;
- acknowledgments, responses, and refusals;
- amendments and successor versions;
- trainee timeline and acknowledgment, response, refusal, and escalation interface;
- append-only audit events; and
- database-enforced immutability.

**Exit:** the full lifecycle is reproducible and mutation attempts fail closed.

## Milestone 5 — Exports and recovery

- deterministic PDF records;
- complete structured exports;
- trainee packet generation;
- attachment integrity;
- retention-policy administration, record holds, lawful disposition, and destruction logs;
- scheduled backup retention;
- restore verification; and
- operator documentation; and
- operator interface for export, hold, disposition, backup, and restore workflows.

**Exit:** a center can leave with all of its data and can prove recovery from a clean installation.

## Milestone 6 — Pilot hardening

- accessibility and usability review;
- keyboard, screen-reader, error-recovery, and responsive-layout proof across every shipped workflow;
- threat modeling;
- privacy review;
- performance and concurrency tests;
- migration compatibility policy;
- deployment packages for common environments; and
- external security assessment.

**Exit:** explicit owner acceptance and real recovery evidence support a production-readiness decision.

## Held until justified

- OIDC;
- optional SMTP notification delivery;
- installation-level signing keys;
- hosted instance management;
- advanced analytics;
- integrations with external personnel systems; and
- configurable custom roles.

These are valid future capabilities. None gets to infect migration `0001` merely because it sounds enterprise-shaped.
