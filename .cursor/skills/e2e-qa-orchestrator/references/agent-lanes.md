# Agent lanes and handoff contract

Adapt these 12 coverage lanes after inventorying the real app, preserving two stress scopes. When a default surface does not exist, assign its lane to a relevant uncovered feature or platform slice. Workers may handle successive lanes within platform capacity; use sequential execution when delegation is unavailable. Keep one editing owner per lane at a time.

## Contents

- [Lane map](#lane-map)
- [Assignment contract](#assignment-contract)
- [Lane autonomy loop](#lane-autonomy-loop)
- [Finding packet](#finding-packet)
- [Severity guide](#severity-guide)
- [Collision protocol](#collision-protocol)
- [Stress packet](#stress-packet)

## Lane map

| Lane | Default responsibility | Primary evidence |
| --- | --- | --- |
| 01 | Build, boot, route inventory, health checks, and golden-path smoke | Reproducible baseline and coverage map |
| 02 | Authentication, onboarding, sessions, account lifecycle, and recovery | State-transition and identity evidence |
| 03 | Primary user journey group A | Browser traces and contract assertions |
| 04 | Primary user journey group B plus secondary/admin flows | Role-aware end-to-end traces |
| 05 | Public/internal APIs and live third-party integrations | Sanitized request IDs, status classes, and contract diffs |
| 06 | Persistence, migrations, caching, queues, jobs, and data lifecycle | Database/state assertions and replay evidence |
| 07 | UI states, responsive layouts, keyboard use, accessibility, and rendering | Screenshots, accessibility output, and interaction traces |
| 08 | Failure recovery, offline/timeout behavior, retries, cancellation, and idempotency | Fault-injection and recovery evidence |
| 09 | Authorization boundaries, tenant isolation, input abuse, and privacy behavior | Safe negative tests and access-control assertions |
| 10 | Gateway/configuration variants, callbacks, process lifecycle, browser/platform compatibility | Cross-service and environment matrix |
| 11 | Bounded service/API load, burst, and soak testing | Throughput, latency percentiles, errors, saturation, recovery |
| 12 | Bounded browser/workflow concurrency, race testing, and regression auditing | Concurrent journey traces, integrity checks, and flaky-test analysis |

Do not send secrets or credential values to agents. Give them variable names and arrange inherited environment or ignored lane-local configuration through the coordinator.

## Assignment contract

Include all of the following in each agent prompt:

```text
Lane: <01-12>
Scope: <features, roles, routes, APIs, or platforms>
Ownership: <files/components the lane may edit>
Worktree: <absolute path>
Branch/base: <branch and immutable SHA>
Ports: gateway=<n>, app=<n>, callback=<n>
Data namespace: <run-id-lane>
Credential variable names: <names only>
Startup/health commands: <repo-derived, redacted>
Test and stress budget: <finite limits and stop conditions>
Required checks: <focused and broader checks>
Authorized actions: <inherit the user's scope: audit, fix, commit, publish PRs/issues, review replies; inherit recorded merge authority, with the coordinator performing merges>
Coordination: do not edit the canonical report; do not revert other agents; escalate shared-file collisions
Process ownership: use only the assigned session handle and ports; never terminate an unknown listener
Command safety: use exact paths and narrow dedicated tools; never disguise or retry a denied operation through a wrapper
Completion: <required evidence and current-SHA checks, finite budgets, publication scope, and stop conditions>
```

Require the agent to return a status update after boot, reproduction, root-cause decision, fix, and authorized publication. Include active session handles, health URLs, log paths, and run-owned resource IDs in boot and cleanup packets so the coordinator can persist them. Raw local artifacts remain available for coordinator verification.

## Lane autonomy loop

Require each lane to continue until its coordinator-defined terminal state:

1. Reconcile its worktree, branch, gateway session, assigned ports, tests, commits, and PR before acting.
2. Execute the next uncovered scenario or advance the oldest reproduced finding.
3. Send a packet immediately after every material state change.
4. Continue independent scenarios while one finding waits for coordinator ownership or external service recovery.
5. Mark a verified draft ready for review, collect Codex and human feedback, fix valid findings, and report evidence to the coordinator. Follow the PR lifecycle for replies and individual thread resolution. The coordinator owns merging and stack order.
6. Stop when the lane is `completed`, is explicitly replaced, reaches its recorded budget, or has an evidence-backed external blocker covering every remaining scenario. On a budget stop, preserve pending work and notify the coordinator; do not call it completed.

If a command is denied, report the intended operation, exact non-secret target, read-only preflight evidence, denial text, and which narrower dedicated tool was attempted. Do not alter the operation to evade the safety layer.

## Finding packet

Require one packet per status change. Use a lane-local candidate ID until the coordinator assigns an `E2E-###` ID. Mark pending packet fields clearly, but omit them when calling the report helper; placeholder text is not evidence.

```text
ID: E2E-###
Lane: ##
Title: <user-visible failure>
Severity: blocker | critical | major | minor
Status: suspected | reproduced | fixing | fixed | verified | deferred | duplicate | excluded
Duplicate of: <ID or n/a>
Environment: <SHA, gateway URL/port, browser/runtime, test namespace>
Preconditions: <minimal state>
Reproduction: <deterministic steps or statistical reproduction rate>
Expected: <documented or code-derived contract>
Actual: <observed result>
Root cause: <specific faulty invariant, code path, or data transition>
Affected variants: <callers, roles, platforms, or endpoints searched>
Regression test: <path and test name, fail-before/pass-after evidence>
Fix: <what changed and why it fully addresses the root cause>
Validation: <commands and outcomes>
Evidence: <sanitized artifact paths, request IDs, screenshots, traces>
Commit: <SHA or n/a>
PR: <URL, current head/base SHAs, review status, and unresolved thread IDs>
Issue: <URL or durable local issue draft for deferred core refactor>
Exclusion reason: <evidence that disproves or puts the candidate outside scope>
Residual risk: <remaining uncertainty>
```

## Severity guide

- `blocker`: prevents broad use, corrupts data, or creates an immediate critical safety/security condition.
- `critical`: breaks a core journey for many users, crosses an authorization boundary, or risks significant data loss.
- `major`: materially breaks a supported journey with no reasonable recovery or violates an important contract.
- `minor`: real and reproducible incorrect behavior with limited impact or a reasonable workaround.

Do not use severity to inflate counts. Observations, product suggestions, expected limitations, test-harness defects, and unconfirmed suspicions belong in the timeline or remaining-risks section, not the bug total.

## Collision protocol

When two lanes need the same file or root cause:

1. Stop the later edit.
2. Exchange evidence through the coordinator.
3. Assign one owning lane for the root-cause fix.
4. Mark other manifestations as duplicates and add their coverage evidence to the owning finding.
5. Let the non-owning lane verify its journey against the owning branch or integration worktree.

## Stress packet

In addition to the finding packet, lanes 11 and 12 must return:

```text
Target and scenario:
Read/write classification:
Concurrency ramp:
Duration and total operations:
Throughput:
p50/p95/p99 latency and sample count:
Errors by class:
Resource saturation:
Recovery time and behavior:
Data-integrity result:
Stop condition triggered:
```
