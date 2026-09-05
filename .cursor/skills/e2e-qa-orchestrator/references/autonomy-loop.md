# Autonomous campaign loop

Use saved report state to resume work. Check current workers, processes, Git state, and GitHub before acting on old records.

## Phase state machine

Advance only after the exit condition is recorded.

| Phase | Required work | Exit condition |
| --- | --- | --- |
| `preflight` | Read instructions, inspect repo, resolve base, verify auth/key names and safety boundaries | Immutable base and capabilities recorded |
| `inventory` | Map features, roles, routes, services, data transitions, integrations, tests, and risk areas | Twelve adapted lane scopes have owners |
| `provisioning` | Create clean worktrees/branches, port plan, namespaces, gateway commands, and report state | Each dispatched lane has isolated launch inputs |
| `functional` | Run lanes 01-10, reproduce, deduplicate, diagnose, and perform authorized fixes/publishing | All functional lanes completed or evidence-blocked |
| `stress` | Run lanes 11-12 under finite budgets and verify recovery/integrity | Both stress lanes completed or evidence-blocked |
| `expansion` | Exercise cross-feature, boundary, race, recovery, and historical-risk combinations | Target reached, honest exhaustion, or explicit budget stop recorded |
| `integration` | Combine verified fixes in a disposable worktree and run cross-lane regression | Integration results linked to every affected finding/PR |
| `convergence` | Run the PR lifecycle through the requested delivery | Review delivery has current reviews and passing checks; merge delivery has landed PRs and passing checks on main |
| `cleanup` | Stop registered sessions, clean run-owned remote data, reconcile worktrees and artifacts | No unknown process or destructive cleanup remains |
| `completed` | Save final report checkpoint and publish when authorized | Terminal checklist passes |

## Coordinator cycle

Repeat this cycle until the terminal checklist passes, the user stops the run, the recorded budget is spent, or every remaining action needs an external change:

1. Load the state file and inspect current platform truth for agents, sessions, worktrees, branches, PRs, issues, and CI.
2. Reconcile stale records. Never infer that an agent, gateway, PR, or test succeeded merely because it was started.
3. Harvest messages and normalize them into finding or lane updates.
4. Deduplicate root causes and resolve ownership collisions before accepting more edits.
5. Verify finished lane work and send focused follow-ups for missing evidence.
6. Dispatch the next pending lane whenever capacity exists.
7. Advance independent fixes and the PR lifecycle: ready, Codex review, comment fixes, thread resolution, checks, merge, and checks on main. Serialize merges that affect the same base or stack.
8. Persist the phase, lane status, findings, and timeline before waiting.
9. Wait for a bounded interval or agent update, then repeat. Do not return control merely because asynchronous work is still running.

Send the user concise progress updates during long cycles. Apply new user constraints and scope changes, including requests to narrow or stop the campaign.

## Decision policy

Proceed without asking when the choice is reversible, local to the scoped repository or run-owned test data, supported by repository evidence, and does not materially change cost or external impact. Record the assumption.

Ask for missing scope or authority only when it matters to remaining work and cannot be inferred from existing instructions. Prepare concrete local patches, drafts, evidence, and cost estimates before asking for publication or other external action. Request credential setup by variable name, never secret values in chat. Continue independent lanes while waiting.

Do not let missing optional tooling stop the campaign. Use repository-native alternatives, preserve local branches and evidence, and record which publishing or external step remains.

## Recovery table

| Condition | Autonomous response | Escalate when |
| --- | --- | --- |
| Agent silent or stale | Inspect status once, send one focused follow-up and allow a bounded response window; stop or confirm the original has stopped before replacing its editing owner | Repeated platform failure prevents both delegated and sequential execution |
| Agent loses context | Rehydrate from report state, worktree diff, commits, logs, and raw artifacts; never rely on conversational memory alone | Required artifact is missing and cannot be reproduced |
| Gateway fails to boot | Capture the first decisive error, verify repo command/config/port, fix a real setup defect, and retry once from the same lane state | Retry would require new authority, production access, or undocumented dependency changes |
| Gateway crashes | Preserve logs and reproduction; treat repeatable app crashes as findings; restart only to continue diagnosis | Crash corrupts shared data or cannot be isolated |
| Port becomes occupied | Use `port_plan.py replan` on the latest full plan, save a new revision, and update that lane's state | The app or provider requires one fixed unavailable port |
| Dependency missing | Use the lockfile and repository-declared install command in the lane worktree; avoid global installs | No lockfile or authoritative install path exists and alternatives materially diverge |
| Test flakes | Fix the seed/environment, repeat enough times to measure a rate, and classify the harness or app root cause | Evidence remains nondeterministic after controlled characterization |
| Two lanes touch one root cause | Stop the later edit, assign one owner, mark other manifestations as duplicates, and make other lanes verify | Ownership cannot be separated without a core refactor |
| Branch conflict | Preserve both commits, resolve only understood campaign changes in an integration worktree, rerun affected tests | Conflict involves unrelated user work or ambiguous product behavior |
| CI fails | Reproduce the failing check locally where feasible, fix branch-caused failures, and recheck the latest SHA; after two failed attempts at the same cause, reassess the diagnosis instead of repeating it | Failure requires external access, scope change, or the declared attempt budget is spent |
| Review feedback arrives | Address valid in-scope comments, explain evidence-bound disagreements, update tests/report, and recheck CI | Requested change expands scope or requires a core-functionality refactor |
| Provider throttles or fails | Stop load, preserve metrics, validate app recovery, and continue non-provider surfaces | All remaining coverage requires the unavailable provider |
| Credential absent or invalid | Continue unauthenticated and non-provider surfaces; record exact variable name and affected scenarios | Every remaining lane requires the credential |
| Command-safety denial | Follow `safe-operations.md`: narrow the target through read-only checks and use a dedicated tool without changing intent | No transparent safe equivalent exists and the denied operation is terminally required |

Never use retries to mask a deterministic failure. Retry only after identifying a transient condition or changing one evidenced setup variable. Record wait/attempt limits for provider and CI operations. On expiry, persist an external blocker or budget stop; do not wait indefinitely for a reviewer or service.

## Coverage expansion below the target

After the primary lanes, run at most three expansion passes unless the user sets another budget. Each pass covers the relevant scenarios from all three classes:

1. Cross-feature pairwise journeys: combine roles, state transitions, and integrations that individual lanes tested separately.
2. Boundary and recovery journeys: empty/maximum inputs, expiry, cancellation, restart, offline/online transitions, partial external failures, and idempotent replay.
3. Concurrency and historical risk: race-prone data changes, repeated callbacks, queue ordering, cache invalidation, migrations, and areas with dense prior fixes or weak tests.

Choose finite scenario lists before each pass and re-rank them using failures and gaps from the preceding pass. Persist attempted scenarios, skipped boundaries, new unique root causes, and the consecutive zero-new-finding count. Stop discovery at the target, or at honest exhaustion when all applicable lanes and critical boundaries have evidence and two consecutive full passes yield no new unique root cause. A blocked boundary cannot satisfy exhaustion.

If the final allowed pass still finds bugs or leaves gaps, record `expansion-budget`, finish already-authorized repair and PR delivery within the overall budget, then use run status `stopped` for the unfinished discovery. Report actual counts and the next scenarios. Do not claim exhaustion or silently add passes. A user deadline takes precedence; checkpoint unfinished work when it arrives.

## Terminal checklist

Require all of the following:

- Twelve lane records are `completed`; no required surface is still blocked. Skipped out-of-scope scenarios have a reason in coverage evidence.
- In repair mode, every counted finding is `verified` or `deferred` with an issue or local issue draft. In audit mode, diagnosed `reproduced` findings may remain unfixed with artifact evidence and root cause. Duplicates link directly to an owning finding; disproven suspicions use `excluded` with a reason.
- Every safe fix has a preserved commit, focused and broader validation, and integration evidence at its current SHA. Review delivery has current CI, completed Codex review, valid approvals, and resolved threads. Merge delivery additionally has confirmed landing on the target branch and passing checks on the landed result. Local delivery has local evidence and drafts.
- Core refactors have evidenced issues or local drafts according to scope.
- Stress and expansion criteria are recorded.
- Required integration regression is complete. A required blocked check prevents completion.
- Registered gateway sessions are stopped; unknown processes are untouched.
- Run-owned remote data is cleaned through exact recorded IDs or retained with paths/IDs and reasons.
- Dirty or uncertain worktrees are preserved; clean worktrees are reconciled through the safe cleanup process.
- The coordinator has recorded checkpoints for `coverage`, `stress`, `expansion`, `integration`, `convergence`, and `cleanup` as `passed` or `not-applicable`, each with evidence. Use `not-applicable` only for a justified scope exclusion, not a failed or unavailable requirement. The helper checks state completeness; the coordinator must inspect the artifacts and current external state.
- The final report checkpoint is saved, published if authorized, and the user handoff names all remaining risks. The report cannot contain its own final commit SHA; record that receipt in the handoff.

Set the report `completed` only when this checklist passes. When it does not, retain resume inputs and use `blocked` for remaining external dependencies or `stopped` for cancellation/budget exhaustion.

## PR supervision

Follow [pr-lifecycle.md](pr-lifecycle.md). Persist PR and thread IDs, head and base commits, review requests, check runs, dependency order, merge receipts, and the next action. Workers can return after lane verification; the coordinator keeps supervising their PRs. A quiet comment stream does not prove review completion. Pending review or CI is a wait state while progress is possible. An unavailable reviewer, exhausted service quota, or permission failure is an external blocker after the documented recovery attempts. Do not shorten requested merge delivery to PR creation or readiness.
