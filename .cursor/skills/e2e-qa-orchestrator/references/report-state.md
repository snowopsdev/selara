# Report state and helper commands

Use this reference when initializing, resuming, recording packets, assigning ports, or closing a campaign. Resolve `SKILL_DIR`, `REPORT_WORKTREE`, `REPO_ROOT`, `BASE_COMMIT`, `RUN_ID`, `TARGET`, and `CAMPAIGN_MODE` before running examples. The default target is 100. Use mode `repair` when fixes are authorized and `audit` for findings-only work. Store each value in the run notes so another coordinator can recover it without conversation history.

For autonomous delivery to main, also set `DELIVERY=merge` and `TARGET_BRANCH=main`. Use `review` when the requested work stops after PR review and `local` for local results. Record existing authority in notes. Omitted delivery defaults to local for compatibility; do not omit it when the user requested merging.

## State ownership and recovery

Only the coordinator writes the canonical report. Serialize helper calls; concurrent read-modify-write operations can lose updates. Each file replacement is atomic, but the JSON and Markdown are not a single transaction. `<run-id>.state.json` is canonical; Markdown is its generated view. If a write stops between the two replacements, recover Markdown with `render`.

Keep the JSON, Markdown, port-plan revisions, coverage matrix, and sanitized artifact inventory together in the reporting worktree. Include exact run-owned remote IDs and session handles in the inventory, but no secrets or customer payloads. The helper does not validate path confinement, artifact existence, Git SHAs, CI truth, or semantic evidence quality. Inspect these separately. Inspect legacy state against the current checklist before resuming; older reports may lack required fields.

```bash
python3 "$SKILL_DIR/scripts/report.py" init \
  --report "$REPORT_WORKTREE/e2e-reports/$RUN_ID.md" \
  --repo "$REPO_ROOT" --run-id "$RUN_ID" \
  --base-commit "$BASE_COMMIT" --target "$TARGET" --mode "$CAMPAIGN_MODE" \
  --delivery "$DELIVERY" --target-branch "$TARGET_BRANCH"

python3 "$SKILL_DIR/scripts/report.py" render --report "$RUN_REPORT"
```

Set `RUN_REPORT` to the initialized absolute report path. Use `status --notes` for scope, capability and publication decisions; notes replace the previous value. Use `event --message` to append evidence or assumptions. Preserve previous notes when updating them.

```bash
python3 "$SKILL_DIR/scripts/report.py" lane \
  --report "$RUN_REPORT" --lane 3 --status running \
  --agent "$AGENT_HANDLE" --worktree "$LANE_WORKTREE" --branch "$LANE_BRANCH" \
  --gateway "$GATEWAY_PORT" --app "$APP_PORT" --callback "$CALLBACK_PORT" \
  --session "$SESSION_HANDLE" --startup "$REDACTED_START_COMMAND" \
  --health-url "$HEALTH_URL" --log-path "$LOG_PATH" --namespace "$LANE_NAMESPACE" \
  --artifacts "$ARTIFACT_INVENTORY" --summary "Readiness verified; journey tests started"
```

In sequential mode, omit `--agent`. `--artifacts` points to an inventory that can list multiple sessions, logs, local files, and remote IDs. Replacing an agent or primary session preserves the previous handle in JSON. Reconcile those histories during cleanup. A blocked lane requires `--blocker` with the exact condition and evidence. Do not change an unfinished lane to completed to satisfy the helper.

## Finding updates

The coordinator assigns canonical IDs. Counted statuses require reproduction, expected behavior, actual behavior, and sanitized artifact evidence. Later statuses also require root cause, fix, regression, validation, or issue evidence as appropriate. Verified findings need a preserved commit before campaign completion. Record meaningful manual regression evidence when an automated test is infeasible.

```bash
python3 "$SKILL_DIR/scripts/report.py" finding \
  --report "$RUN_REPORT" --id E2E-001 --lane 03 --severity major \
  --status reproduced --title "Saved filter is lost after token refresh" \
  --reproduction "Refresh the session while a saved filter is active" \
  --expected "Saved filter remains active" --actual "Filter resets to default" \
  --evidence "lane-03/artifacts/filter-refresh.txt"
```

`--evidence` is repeatable and accumulates unique entries. Other supplied fields replace their previous values. Use `--status duplicate --duplicate-of E2E-001` only for an existing canonical owner. Reassign dependent duplicates before changing their owner to a duplicate or excluding it. Promoting a duplicate back to an active finding clears its duplicate link. Use `--status excluded --exclusion-reason "..."` for a disproven or out-of-scope candidate. Excluded and duplicate findings do not inflate totals.

`--issue` may hold a durable local issue-draft path when publication is outside scope. If publication is required but blocked, the local draft preserves work; it does not satisfy that publishing requirement.

## Port plans

```bash
python3 "$SKILL_DIR/scripts/port_plan.py" plan \
  --base 4200 --lanes 12 --output "$PORT_PLAN"

python3 "$SKILL_DIR/scripts/port_plan.py" check --plan "$PORT_PLAN" --lane 3

python3 "$SKILL_DIR/scripts/port_plan.py" replan \
  --plan "$PORT_PLAN" --lane 3 --output "$NEXT_PORT_PLAN"
```

Use a new `NEXT_PORT_PLAN` path, then record it as the authoritative revision. Replanning retains every other lane's assigned ports, even when those lanes are not running yet. Update the changed lane's state before restart. Checking the whole plan while other lanes are listening will correctly report those ports occupied. The helper probes IPv4 TCP availability on the specified host; verify the app's actual bind address and protocol at startup.

## Completion checkpoints

Record `coverage`, `stress`, `expansion`, `integration`, `convergence`, and `cleanup` separately. For each, use `passed`, `blocked`, or `not-applicable` with a specific evidence path or explanation. A blocked requirement cannot be marked not applicable to bypass completion.

```bash
python3 "$SKILL_DIR/scripts/report.py" checkpoint \
  --report "$RUN_REPORT" --name integration --outcome passed \
  --evidence "$INTEGRATION_EVIDENCE"

python3 "$SKILL_DIR/scripts/report.py" status \
  --report "$RUN_REPORT" --status completed --phase completed \
  --message "Terminal checklist verified; final checkpoint saved"
```

Completion requires all 12 lanes completed, every finding resolved with required fields, and all six checkpoints passed or justifiably not applicable. In `audit` mode, diagnosed `reproduced` findings may remain unfixed; they need reproduction, expected/actual behavior, artifact evidence, and root cause. Repair mode requires verified fixes or evidenced deferrals. These are structural checks, not proof of test success. Both status and phase must be completed together. To resume a closed campaign, explicitly set `--status running` and an active `--phase`, then refresh affected checkpoints after changes. Never reuse old passing checkpoints for changed code.

For a budget stop or cancellation, set `--status stopped`, retain the actual phase, and include the stop reason and next work in notes. For external dependencies, use `blocked`.

## PR snapshots and merge receipts

Use [pr-lifecycle.md](pr-lifecycle.md) to supervise PRs. Save sanitized snapshots in the reporting worktree, then register each with:

```bash
python3 "$SKILL_DIR/scripts/report.py" pr --report "$RUN_REPORT" --file "$PR_SNAPSHOT"
```

The helper replaces the whole snapshot for that PR number, so stale gate fields are not carried into a new snapshot. Required fields are `number`, `url`, `head_sha`, `base_branch`, and `status`. Status is `draft`, `reviewing`, `fixing`, `ready`, `queued`, `merged`, `blocked`, or `superseded`. Record dependency IDs and review-thread IDs as additional fields when needed.

For a merged PR, the full receipt has this shape. Populate each value from observed evidence. Placeholder strings and boolean claims are not proof.

```json
{
  "number": 123,
  "url": "https://github.com/OWNER/REPO/pull/123",
  "head_sha": "reviewed-head-sha",
  "base_branch": "main",
  "target_branch": "main",
  "status": "merged",
  "gate": {
    "head_sha": "reviewed-head-sha",
    "codex_reviewed_sha": "reviewed-head-sha",
    "base_sha": "checked-base-sha",
    "checked_at": "actual-check-time",
    "codex_complete": true,
    "checks_passed": true,
    "validation_passed": true,
    "threads_resolved": true,
    "approvals_satisfied": true,
    "mergeable": true,
    "checks_evidence": "artifacts/pr-123/checks.json",
    "review_evidence": "artifacts/pr-123/reviews.json",
    "threads_evidence": "artifacts/pr-123/threads.json",
    "validation_evidence": "artifacts/pr-123/validation.txt"
  },
  "merge_commit": "actual-merge-commit",
  "merged_at": "actual-merge-time",
  "landed_on_target": true,
  "postmerge_passed": true,
  "postmerge_evidence": "artifacts/pr-123/main-validation.txt"
}
```

Merge delivery cannot complete until every registered PR is merged with current-head review, passing gates, landing evidence, and checks on the target branch. Every verified finding must link to one of those merged PRs. A superseded PR needs a `reason` and `superseded_by` URL pointing to a merged replacement. Queue acceptance or auto-merge enablement does not satisfy this requirement.

Review delivery cannot complete until every registered PR is `ready` or `merged` with current-head Codex review, passing checks, resolved threads, and satisfied approvals. Every verified finding must link to one of those PRs. Landing and post-merge receipts are not required. Draft, reviewing, fixing, queued, and blocked PRs are not complete.

Zero fixes may require no code PRs; explain that outcome in the convergence checkpoint.

Old campaigns load as local delivery. When the user extends a campaign to merging, resume the same state with `status --status running --phase convergence --delivery merge --target-branch main --message "User authorized delivery through main"`. A change to delivery or target needs an explanation. Do not downgrade requested delivery to make completion pass. Reconcile PR evidence and checkpoints; old ready-for-review evidence is not a merge receipt.

## Helper verification

Run after changing the helpers:

```bash
python3 "$SKILL_DIR/scripts/report.py" self-test
python3 "$SKILL_DIR/scripts/port_plan.py" self-test
python3 -m unittest discover -s "$SKILL_DIR/tests" -v
```
