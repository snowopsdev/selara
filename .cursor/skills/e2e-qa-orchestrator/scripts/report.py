#!/usr/bin/env python3
"""Maintain an atomic, sanitized Markdown report for an E2E QA campaign."""

from __future__ import annotations

import argparse
import json
import os
import re
import tempfile
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SEVERITIES = ("blocker", "critical", "major", "minor")
STATUSES = ("suspected", "reproduced", "fixing", "fixed", "verified", "deferred", "duplicate", "excluded")
COUNTED_STATUSES = {"reproduced", "fixing", "fixed", "verified", "deferred"}
PHASES = (
    "preflight", "inventory", "provisioning", "functional", "stress", "expansion",
    "integration", "convergence", "cleanup", "completed",
)
LANE_STATUSES = ("pending", "provisioning", "running", "blocked", "completed", "replaced")
CHECKPOINTS = ("coverage", "stress", "expansion", "integration", "convergence", "cleanup")
LANE_FIELDS = (
    "status", "agent", "worktree", "branch", "gateway", "app", "callback", "summary",
    "session", "startup", "health_url", "log_path", "namespace", "artifacts", "blocker",
)
SECRET_PATTERNS = (
    re.compile(r"(?i)\b(api[_-]?key|access[_-]?token|auth(?:orization)?|secret|password)\s*[:=]\s*([^\s,;]+)"),
    re.compile(r"\b(?:sk|ghp|github_pat|xox[baprs])[-_][A-Za-z0-9_-]{12,}\b"),
    re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{12,}"),
)


def now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sanitize_text(value: str) -> str:
    # Redact the complete header before the generic assignment matcher consumes
    # only the scheme and leaves its credential behind.
    value = re.sub(r"(?im)\b(?:proxy-)?authorization\s*[:=]\s*[^\r\n]+", "Authorization=[REDACTED]", value)
    value = SECRET_PATTERNS[2].sub("Bearer [REDACTED]", value)
    value = SECRET_PATTERNS[0].sub(lambda match: f"{match.group(1)}=[REDACTED]", value)
    value = SECRET_PATTERNS[1].sub("[REDACTED]", value)
    return value


def sanitize(value: Any) -> Any:
    if isinstance(value, str):
        return sanitize_text(value)
    if isinstance(value, list):
        return [sanitize(item) for item in value]
    if isinstance(value, dict):
        return {key: sanitize(item) for key, item in value.items()}
    return value


def state_path(report: Path) -> Path:
    return report.with_suffix(".state.json")


def atomic_write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as handle:
        handle.write(content)
        temporary = Path(handle.name)
    os.replace(temporary, path)


def load_state(report: Path) -> dict[str, Any]:
    path = state_path(report)
    if not path.exists():
        raise SystemExit(f"State file not found: {path}. Run 'init' first.")
    with path.open(encoding="utf-8") as handle:
        state = json.load(handle)
    state.setdefault("phase", "preflight")
    state.setdefault("base_commit", "unknown")
    state.setdefault("lanes", [])
    state.setdefault("findings", [])
    state.setdefault("timeline", [])
    state.setdefault("checkpoints", {})
    state.setdefault("mode", "repair")
    state.setdefault("delivery", "local")
    state.setdefault("target_branch", "main")
    state.setdefault("prs", [])
    return state


def load_mutable_state(report: Path) -> dict[str, Any]:
    data = load_state(report)
    if data["status"] == "completed":
        raise SystemExit("Reopen the campaign with status running and an active phase before changing its work records")
    return data


def save(report: Path, state: dict[str, Any]) -> None:
    state["updated_at"] = now()
    state = sanitize(state)
    atomic_write(state_path(report), json.dumps(state, indent=2, sort_keys=True) + "\n")
    atomic_write(report, render(state))


def format_value(value: Any) -> str:
    if value in (None, "", []):
        return "_Pending._"
    if isinstance(value, list):
        return "\n".join(f"- {item}" for item in value)
    return str(value)


def table_value(value: Any) -> str:
    if value in (None, ""):
        return "—"
    return str(value).replace("|", "\\|").replace("\n", " ")


def render(state: dict[str, Any]) -> str:
    findings = sorted(state["findings"], key=lambda item: item["id"])
    lanes = sorted(state.get("lanes", []), key=lambda item: item["lane"])
    counted = [item for item in findings if item["status"] in COUNTED_STATUSES and not item.get("duplicate_of")]
    status_counts = Counter(item["status"] for item in findings)
    severity_counts = Counter(item["severity"] for item in counted)
    lines = [
        f"# E2E QA Report: {state['run_id']}",
        "",
        "> Generated from the state JSON. Submit sanitized evidence only; automatic redaction is a limited backstop.",
        "",
        "## Run",
        "",
        f"- Repository: `{state['repo']}`",
        f"- Base commit: `{state.get('base_commit', 'unknown')}`",
        f"- Status: **{state['status']}**",
        f"- Phase: **{state.get('phase', 'preflight')}**",
        f"- Mode: {state.get('mode', 'repair')}",
        f"- Delivery: {state.get('delivery', 'local')} to {state.get('target_branch', 'main')}",
        f"- Created: {state['created_at']}",
        f"- Updated: {state['updated_at']}",
        f"- Unique reproduced bugs: **{len(counted)} / {state['target']}**",
        f"- Coverage: {format_value(state.get('coverage'))}",
        "",
        "## Lane status",
        "",
        "| Lane | Status | Agent | Ports | Branch | Summary |",
        "| ---: | --- | --- | --- | --- | --- |",
    ]
    lane_by_id = {item["lane"]: item for item in lanes}
    for lane_number in range(1, 13):
        lane_id = f"{lane_number:02d}"
        lane = lane_by_id.get(lane_id, {"lane": lane_id, "status": "pending"})
        ports = "/".join(str(lane.get(role, "—")) for role in ("gateway", "app", "callback"))
        cells = (
            lane_id,
            lane.get("status"),
            lane.get("agent"),
            ports,
            lane.get("branch"),
            lane.get("summary"),
        )
        lines.append("| " + " | ".join(table_value(value) for value in cells) + " |")
    lines.extend(["", "## Lane recovery details", ""])
    for lane in lanes:
        lines.extend([f"### Lane {lane['lane']}", ""])
        for key in ("worktree", "session", "startup", "health_url", "log_path", "namespace", "artifacts", "blocker"):
            if lane.get(key):
                lines.extend([f"**{key.replace('_', ' ').capitalize()}**", "", format_value(lane[key]), ""])
    lines.extend(["## Completion checkpoints", ""])
    for name in CHECKPOINTS:
        checkpoint = state.get("checkpoints", {}).get(name, {})
        lines.append(f"- {name}: {checkpoint.get('outcome', 'pending')}. {checkpoint.get('evidence', '')}")
    lines.extend([
        "",
        "## Live summary",
        "",
        "| Metric | Count |",
        "| --- | ---: |",
    ])
    for status in STATUSES:
        lines.append(f"| Status: {status} | {status_counts[status]} |")
    for severity in SEVERITIES:
        lines.append(f"| Severity: {severity} | {severity_counts[severity]} |")
    lines.extend(["", "## Pull requests", "", "| PR | Status | Head | Base |", "| --- | --- | --- | --- |"])
    for pr in state.get("prs", []):
        lines.append("| " + " | ".join(table_value(pr.get(key)) for key in ("url", "status", "head_sha", "base_branch")) + " |")
    lines.extend(["", "## Findings", ""])
    if not findings:
        lines.extend(["_No reproduced findings yet._", ""])
    fields = (
        ("lane", "Lane"),
        ("severity", "Severity"),
        ("status", "Status"),
        ("duplicate_of", "Duplicate of"),
        ("environment", "Environment"),
        ("preconditions", "Preconditions"),
        ("reproduction", "Reproduction"),
        ("expected", "Expected"),
        ("actual", "Actual"),
        ("root_cause", "Root cause"),
        ("affected", "Affected variants"),
        ("regression", "Regression test"),
        ("fix", "Fix"),
        ("validation", "Validation"),
        ("evidence", "Evidence"),
        ("commit", "Commit"),
        ("pr", "PR"),
        ("issue", "Issue"),
        ("residual_risk", "Residual risk"),
        ("exclusion_reason", "Exclusion reason"),
    )
    for finding in findings:
        lines.extend([f"### {finding['id']}: {finding['title']}", ""])
        for key, label in fields:
            lines.extend([f"**{label}**", "", format_value(finding.get(key)), ""])
    lines.extend(["## Timeline", ""])
    if state["timeline"]:
        for event in state["timeline"]:
            lines.append(f"- {event['at']} — {event['message']}")
    else:
        lines.append("_No events recorded yet._")
    lines.extend(["", "## Notes", "", format_value(state.get("notes")), ""])
    return "\n".join(lines)


def init_command(args: argparse.Namespace) -> None:
    report = Path(args.report).expanduser().resolve()
    state = state_path(report)
    if report.exists() or state.exists():
        raise SystemExit(f"Refusing to overwrite existing campaign: {report}")
    if args.target < 1:
        raise SystemExit("--target must be positive")
    timestamp = now()
    data = {
        "run_id": args.run_id,
        "repo": str(Path(args.repo).expanduser().resolve()),
        "base_commit": args.base_commit,
        "target": args.target,
        "mode": getattr(args, "mode", "repair"),
        "delivery": getattr(args, "delivery", "local"),
        "target_branch": getattr(args, "target_branch", "main"),
        "prs": [],
        "status": "running",
        "phase": "preflight",
        "coverage": "Inventory in progress",
        "notes": "Live credential values are intentionally excluded from this report.",
        "created_at": timestamp,
        "updated_at": timestamp,
        "lanes": [],
        "findings": [],
        "checkpoints": {},
        "timeline": [{"at": timestamp, "message": "Campaign initialized"}],
    }
    save(report, data)
    print(report)


def event_command(args: argparse.Namespace) -> None:
    report = Path(args.report).expanduser().resolve()
    data = load_state(report)
    data["timeline"].append({"at": now(), "message": args.message})
    save(report, data)
    print(report)


def lane_command(args: argparse.Namespace) -> None:
    report = Path(args.report).expanduser().resolve()
    data = load_mutable_state(report)
    lane_id = f"{args.lane:02d}"
    existing = next((item for item in data["lanes"] if item["lane"] == lane_id), None)
    lane = existing or {"lane": lane_id, "created_at": now()}
    for key in LANE_FIELDS:
        value = getattr(args, key, None)
        if value is not None:
            if key == "agent" and lane.get(key) and lane[key] != value:
                lane.setdefault("previous_agents", []).append(lane[key])
            if key == "session" and lane.get(key) and lane[key] != value:
                lane.setdefault("previous_sessions", []).append(lane[key])
            lane[key] = value
    lane.setdefault("status", "pending")
    if lane["status"] == "blocked" and not lane.get("blocker"):
        raise SystemExit("A blocked lane requires --blocker with evidence")
    lane["updated_at"] = now()
    if existing is None:
        data["lanes"].append(lane)
    data["timeline"].append({"at": now(), "message": f"Lane {lane_id} updated to {lane['status']}"})
    save(report, data)
    print(report)


def finding_command(args: argparse.Namespace) -> None:
    report = Path(args.report).expanduser().resolve()
    data = load_mutable_state(report)
    existing = next((item for item in data["findings"] if item["id"] == args.id), None)
    if existing is None and not args.title:
        raise SystemExit("--title is required for a new finding")
    if existing is None and not args.lane:
        raise SystemExit("--lane is required for a new finding")
    if existing is None and not args.severity:
        raise SystemExit("--severity is required for a new finding")
    if existing is None and not args.status:
        raise SystemExit("--status is required for a new finding")
    finding = existing or {"id": args.id, "created_at": now(), "evidence": []}
    for key in (
        "title", "lane", "severity", "status", "duplicate_of", "environment", "preconditions",
        "reproduction", "expected", "actual", "root_cause", "affected", "regression", "fix",
        "validation", "commit", "pr", "issue", "residual_risk", "exclusion_reason",
    ):
        value = getattr(args, key, None)
        if value is not None:
            finding[key] = value
    if args.evidence:
        finding["evidence"] = list(dict.fromkeys(finding.get("evidence", []) + args.evidence))
    finding.setdefault("severity", "major")
    finding.setdefault("status", "suspected")
    finding["updated_at"] = now()
    if finding["status"] == "duplicate":
        owner = next((item for item in data["findings"] if item["id"] == finding.get("duplicate_of")), None)
        if owner is None or owner["id"] == finding["id"] or owner["status"] in {"duplicate", "excluded"}:
            raise SystemExit("A duplicate requires an existing, distinct, non-duplicate, non-excluded owner")
        if any(item.get("duplicate_of") == finding["id"] for item in data["findings"]):
            raise SystemExit("Reassign dependent duplicates before making their owner a duplicate")
    else:
        if args.duplicate_of:
            raise SystemExit("--duplicate-of applies only to status duplicate")
        finding.pop("duplicate_of", None)
    if finding["status"] == "excluded":
        if not finding.get("exclusion_reason"):
            raise SystemExit("Status excluded requires --exclusion-reason")
        if any(item.get("duplicate_of") == finding["id"] for item in data["findings"]):
            raise SystemExit("Reassign dependent duplicates before excluding their owner")
    if finding["status"] in COUNTED_STATUSES:
        missing = [key for key in ("reproduction", "expected", "actual", "evidence") if not finding.get(key)]
        if missing:
            raise SystemExit(f"Status {finding['status']} requires: {', '.join(missing)}")
    if finding["status"] in {"fixing", "fixed", "verified", "deferred"} and not finding.get("root_cause"):
        raise SystemExit(f"Status {finding['status']} requires a root cause")
    if finding["status"] in {"fixed", "verified"} and not finding.get("fix"):
        raise SystemExit(f"Status {finding['status']} requires a fix")
    if finding["status"] == "verified" and not finding.get("regression"):
        raise SystemExit("Status verified requires regression-test evidence")
    if finding["status"] == "verified" and not finding.get("validation"):
        raise SystemExit("Status verified requires broader validation evidence")
    if finding["status"] == "deferred" and not finding.get("issue"):
        raise SystemExit("Status deferred requires a linked issue")
    if existing is None:
        data["findings"].append(finding)
    data["timeline"].append({"at": now(), "message": f"{finding['id']} updated to {finding['status']}"})
    save(report, data)
    print(report)


def validate_completion(data: dict[str, Any]) -> None:
    """Check recorded state, not the truth of external tests or CI."""
    lanes = {item["lane"]: item for item in data["lanes"]}
    unfinished = [f"{number:02d}" for number in range(1, 13)
                  if lanes.get(f"{number:02d}", {}).get("status") != "completed"]
    if unfinished:
        raise SystemExit("Incomplete lanes: " + ", ".join(unfinished))
    for finding in data["findings"]:
        status = finding["status"]
        required = {
            "verified": ("reproduction", "expected", "actual", "evidence", "root_cause", "fix", "regression", "validation", "commit"),
            "deferred": ("reproduction", "expected", "actual", "evidence", "root_cause", "issue"),
            "duplicate": ("duplicate_of",),
            "excluded": ("exclusion_reason",),
        }
        if data.get("mode", "repair") == "audit":
            required["reproduced"] = ("reproduction", "expected", "actual", "evidence", "root_cause")
        if status not in required or any(not finding.get(key) for key in required[status]):
            raise SystemExit(f"Unresolved or incomplete finding: {finding['id']}")
        if status == "duplicate":
            owner = next((item for item in data["findings"] if item["id"] == finding["duplicate_of"]), None)
            owner_statuses = {"verified", "deferred"}
            if data.get("mode", "repair") == "audit":
                owner_statuses.add("reproduced")
            if owner is None or owner["id"] == finding["id"] or owner["status"] not in owner_statuses:
                raise SystemExit(f"Invalid duplicate owner: {finding['id']}")
    for name in CHECKPOINTS:
        checkpoint = data.get("checkpoints", {}).get(name, {})
        if checkpoint.get("outcome") not in {"passed", "not-applicable"} or not checkpoint.get("evidence"):
            raise SystemExit(f"Incomplete checkpoint: {name}")
    if data.get("delivery") == "merge":
        validate_merge_delivery(data)


def validate_merge_delivery(data: dict[str, Any]) -> None:
    """Validate recorded merge receipts; the coordinator must verify GitHub truth."""
    prs = data.get("prs", [])
    merged = {pr["url"]: pr for pr in prs if pr["status"] == "merged"}
    for finding in data["findings"]:
        if finding["status"] == "verified" and finding.get("pr") not in merged:
            raise SystemExit(f"Finding lacks a merged PR: {finding['id']}")
    for pr in prs:
        if pr["status"] == "superseded":
            if not pr.get("reason") or pr.get("superseded_by") not in merged:
                raise SystemExit(f"Unresolved replacement for PR {pr['number']}")
            continue
        if pr["status"] != "merged":
            raise SystemExit(f"PR {pr['number']} is not merged")
        gate = pr.get("gate", {})
        if not isinstance(gate, dict):
            raise SystemExit(f"Invalid merge evidence for PR {pr['number']}")
        if not pr.get("head_sha") or gate.get("head_sha") != pr["head_sha"] or gate.get("codex_reviewed_sha") != pr["head_sha"]:
            raise SystemExit(f"Stale head or Codex review for PR {pr['number']}")
        for field in ("codex_complete", "checks_passed", "validation_passed", "threads_resolved", "approvals_satisfied", "mergeable"):
            if gate.get(field) is not True:
                raise SystemExit(f"Merge gate {field} not satisfied for PR {pr['number']}")
        for field in ("base_sha", "checked_at", "checks_evidence", "review_evidence", "threads_evidence", "validation_evidence"):
            if not gate.get(field):
                raise SystemExit(f"Missing {field} for PR {pr['number']}")
        if pr.get("target_branch") != data.get("target_branch", "main"):
            raise SystemExit(f"Wrong merge target for PR {pr['number']}")
        if pr.get("landed_on_target") is not True or pr.get("postmerge_passed") is not True:
            raise SystemExit(f"Landing or post-merge validation missing for PR {pr['number']}")
        if any(not pr.get(field) for field in ("merge_commit", "merged_at", "postmerge_evidence")):
            raise SystemExit(f"Incomplete merge receipt for PR {pr['number']}")


def pr_command(args: argparse.Namespace) -> None:
    report = Path(args.report).expanduser().resolve()
    data = load_mutable_state(report)
    with Path(args.file).expanduser().open(encoding="utf-8") as handle:
        pr = json.load(handle)
    if not isinstance(pr, dict) or type(pr.get("number")) is not int or pr["number"] < 1:
        raise SystemExit("PR record needs a positive integer number")
    if pr.get("status") not in {"draft", "reviewing", "fixing", "ready", "queued", "merged", "blocked", "superseded"}:
        raise SystemExit("Invalid PR status")
    if any(not isinstance(pr.get(key), str) or not pr[key].strip() for key in ("url", "head_sha", "base_branch")):
        raise SystemExit("PR record needs url, head_sha, and base_branch")
    # Replace the whole snapshot so old head-specific approvals do not linger.
    pr["updated_at"] = now()
    existing = next((item for item in data["prs"] if item["number"] == pr["number"]), None)
    if existing and existing["url"] != pr["url"]:
        raise SystemExit("PR number already belongs to a different URL")
    data["prs"] = [item for item in data["prs"] if item["number"] != pr["number"]] + [pr]
    data["timeline"].append({"at": now(), "message": f"PR {pr['number']} updated to {pr['status']} at {pr['head_sha']}"})
    save(report, data)
    print(report)


def checkpoint_command(args: argparse.Namespace) -> None:
    report = Path(args.report).expanduser().resolve()
    data = load_mutable_state(report)
    data["checkpoints"][args.name] = {"outcome": args.outcome, "evidence": args.evidence, "updated_at": now()}
    data["timeline"].append({"at": now(), "message": f"Checkpoint {args.name}: {args.outcome}"})
    save(report, data)
    print(report)


def render_command(args: argparse.Namespace) -> None:
    report = Path(args.report).expanduser().resolve()
    atomic_write(report, render(sanitize(load_state(report))))
    print(report)


def run_command(args: argparse.Namespace) -> None:
    report = Path(args.report).expanduser().resolve()
    data = load_state(report)
    for key in ("delivery", "target_branch"):
        value = getattr(args, key, None)
        if value is not None and value != data[key]:
            if not args.message:
                raise SystemExit("Changing delivery or target requires --message recording the user's scope change")
            data[key] = value
    if args.status is not None:
        data["status"] = args.status
    if args.phase is not None:
        data["phase"] = args.phase
    if args.coverage is not None:
        data["coverage"] = args.coverage
    if args.notes is not None:
        data["notes"] = args.notes
    if data["status"] == "completed" or data["phase"] == "completed":
        if data["status"] != "completed" or data["phase"] != "completed":
            raise SystemExit("Set --status completed and --phase completed together")
        validate_completion(data)
    data["timeline"].append({"at": now(), "message": args.message or "Run metadata updated"})
    save(report, data)
    print(report)


def self_test_command(_: argparse.Namespace) -> None:
    with tempfile.TemporaryDirectory() as directory:
        report = Path(directory) / "e2e-reports" / "test-run.md"
        init_command(argparse.Namespace(
            report=str(report), repo=directory, run_id="test-run", base_commit="abc1234",
            target=100,
        ))
        lane_command(argparse.Namespace(
            report=str(report), lane=5, status="running", agent="agent-05", worktree="/tmp/lane-05",
            branch="codex/e2e-test-lane-05", gateway=4250, app=4251, callback=4252,
            summary="Integration tests running",
        ))
        dummy_secret = "sample-" + "credential-value"
        dummy_bearer = "abcdefghijkl" + "mnop"
        finding_args = argparse.Namespace(
            report=str(report), id="E2E-001", title="Token redaction", lane="05", severity="major",
            status="verified", duplicate_of=None, environment=f"API_KEY={dummy_secret}", preconditions="ready",
            reproduction="send request", expected="redacted report", actual="token exposed", root_cause="unsafe logging",
            affected="report output", regression="test_redaction passes", fix="sanitize all report fields",
            validation="self-test", evidence=[f"Bearer {dummy_bearer}"], commit=None, pr=None, issue=None,
            residual_risk="none",
        )
        finding_command(finding_args)
        content = report.read_text(encoding="utf-8")
        assert dummy_secret not in content
        assert dummy_bearer not in content
        assert "1 / 100" in content
        assert "E2E-001" in content
        assert "agent-05" in content
        assert "abc1234" in content
        invalid_args = argparse.Namespace(**vars(finding_args))
        invalid_args.id = "E2E-002"
        invalid_args.title = "Unsubstantiated count"
        invalid_args.status = "reproduced"
        invalid_args.reproduction = None
        try:
            finding_command(invalid_args)
        except SystemExit as error:
            assert "reproduction" in str(error)
        else:
            raise AssertionError("A reproduced finding without reproduction evidence was accepted")
    print("report.py self-test passed")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    init_parser = subparsers.add_parser("init", help="initialize a campaign report")
    init_parser.add_argument("--report", required=True)
    init_parser.add_argument("--repo", required=True)
    init_parser.add_argument("--run-id", required=True)
    init_parser.add_argument("--base-commit", default="unknown")
    init_parser.add_argument("--target", type=int, default=100)
    init_parser.add_argument("--mode", choices=("repair", "audit"), default="repair")
    init_parser.add_argument("--delivery", choices=("local", "review", "merge"), default="local")
    init_parser.add_argument("--target-branch", default="main")
    init_parser.set_defaults(func=init_command)

    event_parser = subparsers.add_parser("event", help="append a timeline event")
    event_parser.add_argument("--report", required=True)
    event_parser.add_argument("--message", required=True)
    event_parser.set_defaults(func=event_command)

    pr_parser = subparsers.add_parser("pr", help="replace a PR snapshot from a sanitized JSON file")
    pr_parser.add_argument("--report", required=True)
    pr_parser.add_argument("--file", required=True)
    pr_parser.set_defaults(func=pr_command)

    lane_parser = subparsers.add_parser("lane", help="create or update a lane record")
    lane_parser.add_argument("--report", required=True)
    lane_parser.add_argument("--lane", type=int, choices=range(1, 13), required=True)
    lane_parser.add_argument("--status", choices=LANE_STATUSES)
    lane_parser.add_argument("--agent")
    lane_parser.add_argument("--worktree")
    lane_parser.add_argument("--branch")
    lane_parser.add_argument("--gateway", type=int)
    lane_parser.add_argument("--app", type=int)
    lane_parser.add_argument("--callback", type=int)
    lane_parser.add_argument("--summary")
    for field in ("session", "startup", "health-url", "log-path", "namespace", "artifacts", "blocker"):
        lane_parser.add_argument("--" + field)
    lane_parser.set_defaults(func=lane_command)

    finding_parser = subparsers.add_parser("finding", help="create or update a finding")
    finding_parser.add_argument("--report", required=True)
    finding_parser.add_argument("--id", required=True)
    finding_parser.add_argument("--title")
    finding_parser.add_argument("--lane")
    finding_parser.add_argument("--severity", choices=SEVERITIES)
    finding_parser.add_argument("--status", choices=STATUSES)
    finding_parser.add_argument("--duplicate-of")
    finding_parser.add_argument("--environment")
    finding_parser.add_argument("--preconditions")
    finding_parser.add_argument("--reproduction")
    finding_parser.add_argument("--expected")
    finding_parser.add_argument("--actual")
    finding_parser.add_argument("--root-cause")
    finding_parser.add_argument("--affected")
    finding_parser.add_argument("--regression")
    finding_parser.add_argument("--fix")
    finding_parser.add_argument("--validation")
    finding_parser.add_argument("--evidence", action="append")
    finding_parser.add_argument("--commit")
    finding_parser.add_argument("--pr")
    finding_parser.add_argument("--issue")
    finding_parser.add_argument("--residual-risk")
    finding_parser.add_argument("--exclusion-reason")
    finding_parser.set_defaults(func=finding_command)

    run_parser = subparsers.add_parser(
        "run", aliases=["status"], help="update run status, coverage, or notes"
    )
    run_parser.add_argument("--report", required=True)
    run_parser.add_argument("--status", choices=("running", "completed", "blocked", "stopped"))
    run_parser.add_argument("--phase", choices=PHASES)
    run_parser.add_argument("--coverage")
    run_parser.add_argument("--notes")
    run_parser.add_argument("--message")
    run_parser.add_argument("--delivery", choices=("local", "review", "merge"))
    run_parser.add_argument("--target-branch")
    run_parser.set_defaults(func=run_command)

    checkpoint_parser = subparsers.add_parser("checkpoint", help="record evidence for a completion condition")
    checkpoint_parser.add_argument("--report", required=True)
    checkpoint_parser.add_argument("--name", choices=CHECKPOINTS, required=True)
    checkpoint_parser.add_argument("--outcome", choices=("passed", "blocked", "not-applicable"), required=True)
    checkpoint_parser.add_argument("--evidence", required=True)
    checkpoint_parser.set_defaults(func=checkpoint_command)

    render_parser = subparsers.add_parser("render", help="recover Markdown from canonical state JSON")
    render_parser.add_argument("--report", required=True)
    render_parser.set_defaults(func=render_command)

    test_parser = subparsers.add_parser("self-test", help="run an isolated smoke test")
    test_parser.set_defaults(func=self_test_command)
    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
