"""Prevent a campaign from finishing before reviewed changes land and pass."""

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from test_helpers import report


def merged_pr():
    return {
        "number": 1, "url": "https://github.com/example/fixture/pull/1",
        "status": "merged", "head_sha": "a" * 40, "base_branch": "main", "target_branch": "main",
        "gate": {
            "head_sha": "a" * 40, "codex_reviewed_sha": "a" * 40,
            "base_sha": "b" * 40, "checked_at": "2026-09-05T12:00:00Z",
            "codex_complete": True, "checks_passed": True, "validation_passed": True,
            "threads_resolved": True, "approvals_satisfied": True, "mergeable": True,
            "checks_evidence": "fixture/checks", "review_evidence": "fixture/reviews",
            "threads_evidence": "fixture/threads", "validation_evidence": "fixture/validation",
        },
        "merge_commit": "c" * 40, "merged_at": "2026-09-05T12:01:00Z",
        "landed_on_target": True, "postmerge_passed": True, "postmerge_evidence": "fixture/main-checks",
    }


def reviewed_pr():
    pr = merged_pr()
    pr["status"] = "ready"
    for key in ("merge_commit", "merged_at", "landed_on_target", "postmerge_passed", "postmerge_evidence"):
        pr.pop(key, None)
    pr["gate"].pop("mergeable", None)
    return pr


class MergeDeliveryTests(unittest.TestCase):
    def state(self, pr):
        return {"findings": [], "prs": [pr], "target_branch": "main"}

    def test_ready_and_queued_are_not_merged(self):
        for status in ("draft", "reviewing", "fixing", "ready", "queued", "blocked"):
            pr = merged_pr()
            pr["status"] = status
            with self.subTest(status=status), self.assertRaisesRegex(SystemExit, "not merged"):
                report.validate_merge_delivery(self.state(pr))

    def test_review_and_gate_must_match_the_landed_head(self):
        for field in ("head_sha", "codex_reviewed_sha"):
            pr = merged_pr()
            pr["gate"][field] = "d" * 40
            with self.subTest(field=field), self.assertRaisesRegex(SystemExit, "Stale"):
                report.validate_merge_delivery(self.state(pr))

    def test_every_merge_condition_must_pass(self):
        for field in ("codex_complete", "checks_passed", "validation_passed", "threads_resolved", "approvals_satisfied", "mergeable"):
            for value in (False, None, "true"):
                pr = merged_pr()
                pr["gate"][field] = value
                with self.subTest(field=field, value=value), self.assertRaisesRegex(SystemExit, field):
                    report.validate_merge_delivery(self.state(pr))

    def test_merge_to_feature_branch_does_not_count_as_main(self):
        pr = merged_pr()
        pr["target_branch"] = "feature-parent"
        with self.assertRaisesRegex(SystemExit, "Wrong merge target"):
            report.validate_merge_delivery(self.state(pr))

    def test_failed_postmerge_checks_keep_campaign_open(self):
        pr = merged_pr()
        pr["postmerge_passed"] = False
        with self.assertRaisesRegex(SystemExit, "post-merge"):
            report.validate_merge_delivery(self.state(pr))
        pr["postmerge_passed"] = True
        del pr["merge_commit"]
        with self.assertRaisesRegex(SystemExit, "receipt"):
            report.validate_merge_delivery(self.state(pr))

    def test_verified_findings_need_a_merged_pr(self):
        state = self.state(merged_pr())
        state["findings"] = [{"id": "E2E-001", "status": "verified", "pr": "missing"}]
        with self.assertRaisesRegex(SystemExit, "lacks a merged PR"):
            report.validate_merge_delivery(state)

    def test_superseded_pr_requires_a_merged_replacement(self):
        state = self.state(merged_pr())
        state["prs"].append({"number": 2, "url": "https://github.com/example/fixture/pull/2", "status": "superseded", "reason": "Replaced"})
        with self.assertRaisesRegex(SystemExit, "replacement"):
            report.validate_merge_delivery(state)
        state["prs"][1]["superseded_by"] = state["prs"][0]["url"]
        report.validate_merge_delivery(state)

    def test_cli_records_delivery_and_replaces_stale_snapshots(self):
        with tempfile.TemporaryDirectory() as directory:
            path = (Path(directory) / "run.md").resolve()
            snapshot = Path(directory) / "pr.json"

            def cli(command, *arguments):
                args = report.build_parser().parse_args([command, "--report", str(path), *arguments])
                with contextlib.redirect_stdout(io.StringIO()):
                    args.func(args)

            cli("init", "--repo", directory, "--run-id", "merge-fixture", "--delivery", "merge", "--target-branch", "main")
            for number in range(1, 13):
                cli("lane", "--lane", str(number), "--status", "completed")
            for name in report.CHECKPOINTS:
                cli("checkpoint", "--name", name, "--outcome", "passed", "--evidence", "fixture/" + name)
            pr = merged_pr()
            snapshot.write_text(json.dumps(pr))
            cli("pr", "--file", str(snapshot))
            changed = {key: value for key, value in pr.items() if key in ("number", "url", "base_branch")}
            changed.update(head_sha="d" * 40, status="reviewing")
            snapshot.write_text(json.dumps(changed))
            cli("pr", "--file", str(snapshot))
            self.assertNotIn("gate", report.load_state(path)["prs"][0])
            with self.assertRaisesRegex(SystemExit, "not merged"):
                cli("status", "--status", "completed", "--phase", "completed")
            snapshot.write_text(json.dumps(pr))
            cli("pr", "--file", str(snapshot))
            cli("status", "--status", "completed", "--phase", "completed")
            self.assertEqual("merge", report.load_state(path)["delivery"])
            self.assertEqual("completed", report.load_state(path)["status"])

    def test_resume_can_add_authorized_merge_delivery_without_losing_state(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "run.md"

            def cli(command, *arguments):
                args = report.build_parser().parse_args([command, "--report", str(path), *arguments])
                with contextlib.redirect_stdout(io.StringIO()):
                    args.func(args)

            cli("init", "--repo", directory, "--run-id", "legacy")
            cli("lane", "--lane", "1", "--status", "completed")
            with self.assertRaisesRegex(SystemExit, "scope change"):
                cli("status", "--delivery", "merge")
            cli("status", "--delivery", "merge", "--phase", "convergence", "--message", "User authorized delivery to main")
            state = report.load_state(path)
            self.assertEqual("merge", state["delivery"])
            self.assertEqual("completed", state["lanes"][0]["status"])


class ReviewDeliveryTests(unittest.TestCase):
    def state(self, pr):
        return {"findings": [], "prs": [pr], "target_branch": "main", "delivery": "review"}

    def test_incomplete_review_statuses_are_rejected(self):
        for status in ("draft", "reviewing", "fixing", "queued", "blocked"):
            pr = reviewed_pr()
            pr["status"] = status
            with self.subTest(status=status), self.assertRaisesRegex(SystemExit, "review is not complete"):
                report.validate_review_delivery(self.state(pr))

    def test_ready_without_a_gate_cannot_complete(self):
        pr = reviewed_pr()
        pr["gate"] = {}
        with self.assertRaisesRegex(SystemExit, "Stale"):
            report.validate_review_delivery(self.state(pr))

    def test_review_gate_must_match_the_current_head(self):
        for field in ("head_sha", "codex_reviewed_sha"):
            pr = reviewed_pr()
            pr["gate"][field] = "d" * 40
            with self.subTest(field=field), self.assertRaisesRegex(SystemExit, "Stale"):
                report.validate_review_delivery(self.state(pr))

    def test_every_review_condition_must_pass(self):
        for field in ("codex_complete", "checks_passed", "validation_passed", "threads_resolved", "approvals_satisfied"):
            for value in (False, None, "true"):
                pr = reviewed_pr()
                pr["gate"][field] = value
                with self.subTest(field=field, value=value), self.assertRaisesRegex(SystemExit, field):
                    report.validate_review_delivery(self.state(pr))

    def test_verified_findings_need_a_reviewed_pr(self):
        state = self.state(reviewed_pr())
        state["findings"] = [{"id": "E2E-001", "status": "verified", "pr": "missing"}]
        with self.assertRaisesRegex(SystemExit, "lacks a reviewed PR"):
            report.validate_review_delivery(state)

    def test_review_delivery_does_not_require_landing(self):
        report.validate_review_delivery(self.state(reviewed_pr()))

    def test_cli_review_delivery_rejects_missing_review_receipts(self):
        with tempfile.TemporaryDirectory() as directory:
            path = (Path(directory) / "run.md").resolve()
            snapshot = Path(directory) / "pr.json"

            def cli(command, *arguments):
                args = report.build_parser().parse_args([command, "--report", str(path), *arguments])
                with contextlib.redirect_stdout(io.StringIO()):
                    args.func(args)

            cli("init", "--repo", directory, "--run-id", "review-fixture", "--delivery", "review", "--target-branch", "main")
            for number in range(1, 13):
                cli("lane", "--lane", str(number), "--status", "completed")
            for name in report.CHECKPOINTS:
                cli("checkpoint", "--name", name, "--outcome", "passed", "--evidence", "fixture/" + name)
            with self.assertRaisesRegex(SystemExit, "lacks a reviewed PR"):
                self._complete_with_verified_finding(cli)
            snapshot.write_text(json.dumps(reviewed_pr()))
            cli("pr", "--file", str(snapshot))
            cli("finding", "--id", "E2E-001", "--lane", "01", "--title", "Synthetic fixture",
                "--severity", "major", "--status", "verified",
                "--reproduction", "Run controlled fixture", "--expected", "Saved value", "--actual", "Lost value",
                "--evidence", "fixture/trace", "--root-cause", "State reset", "--fix", "Preserve state",
                "--regression", "Fails before, passes after", "--validation", "Journey passed",
                "--commit", "b" * 40, "--pr", reviewed_pr()["url"])
            cli("status", "--status", "completed", "--phase", "completed")
            self.assertEqual("review", report.load_state(path)["delivery"])
            self.assertEqual("completed", report.load_state(path)["status"])

    def _complete_with_verified_finding(self, cli):
        cli("finding", "--id", "E2E-001", "--lane", "01", "--title", "Synthetic fixture",
            "--severity", "major", "--status", "verified",
            "--reproduction", "Run controlled fixture", "--expected", "Saved value", "--actual", "Lost value",
            "--evidence", "fixture/trace", "--root-cause", "State reset", "--fix", "Preserve state",
            "--regression", "Fails before, passes after", "--validation", "Journey passed",
            "--commit", "b" * 40)
        cli("status", "--status", "completed", "--phase", "completed")


if __name__ == "__main__":
    unittest.main()
