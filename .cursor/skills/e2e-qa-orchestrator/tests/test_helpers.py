"""Behavioral regressions for campaign reporting and port recovery."""

import contextlib
import copy
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


def import_script(name):
    path = Path(__file__).resolve().parents[1] / "scripts" / (name + ".py")
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


report = import_script("report")
ports = import_script("port_plan")


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = (Path(self.directory.name) / "campaign.md").resolve()
        self.cli("init", "--repo", self.directory.name, "--run-id", "fixture", "--base-commit", "a" * 40)

    def cli(self, command, *arguments):
        args = report.build_parser().parse_args([command, "--report", str(self.path), *arguments])
        with contextlib.redirect_stdout(io.StringIO()):
            args.func(args)

    def finding(self, finding_id="E2E-001", status="reproduced", *extra):
        self.cli("finding", "--id", finding_id, "--lane", "01", "--title", "Synthetic fixture",
                 "--severity", "major", "--status", status,
                 "--reproduction", "Run controlled fixture", "--expected", "Saved value", "--actual", "Lost value",
                 *extra)

    def resolved_lanes(self):
        for number in range(1, 13):
            self.cli("lane", "--lane", str(number), "--status", "completed")

    def checkpoints(self):
        for name in report.CHECKPOINTS:
            self.cli("checkpoint", "--name", name, "--outcome", "passed", "--evidence", "fixture/" + name)

    def complete(self):
        self.cli("status", "--status", "completed", "--phase", "completed")

    def test_empty_campaign_cannot_complete_or_change_phase_only(self):
        before = report.state_path(self.path).read_bytes()
        with self.assertRaisesRegex(SystemExit, "Incomplete lanes"):
            self.complete()
        with self.assertRaisesRegex(SystemExit, "together"):
            self.cli("status", "--phase", "completed")
        self.assertEqual(before, report.state_path(self.path).read_bytes())

    def test_counted_findings_require_artifact_evidence(self):
        with self.assertRaisesRegex(SystemExit, "evidence"):
            self.finding()
        self.assertEqual([], report.load_state(self.path)["findings"])
        self.finding("E2E-001", "reproduced", "--evidence", "fixture/reproduction.txt")
        self.assertIn("1 / 100", self.path.read_text())

    def test_duplicate_owner_must_exist_and_be_distinct(self):
        for owner in ("E2E-999", "E2E-001"):
            with self.subTest(owner=owner), self.assertRaisesRegex(SystemExit, "owner"):
                self.finding("E2E-001", "duplicate", "--duplicate-of", owner)

    def test_duplicate_chains_and_owner_exclusion_are_rejected(self):
        self.finding("E2E-001", "reproduced", "--evidence", "fixture/trace")
        self.finding("E2E-002", "duplicate", "--duplicate-of", "E2E-001")
        with self.assertRaisesRegex(SystemExit, "owner"):
            self.finding("E2E-003", "duplicate", "--duplicate-of", "E2E-002")
        with self.assertRaisesRegex(SystemExit, "dependent duplicates"):
            self.cli("finding", "--id", "E2E-001", "--status", "excluded", "--exclusion-reason", "Outside scope")
        self.assertIn("1 / 100", self.path.read_text())

    def test_duplicate_promotion_restores_count(self):
        self.finding("E2E-001", "reproduced", "--evidence", "fixture/trace-1")
        self.finding("E2E-002", "duplicate", "--duplicate-of", "E2E-001")
        self.cli("finding", "--id", "E2E-002", "--status", "reproduced", "--evidence", "fixture/trace-2")
        state = report.load_state(self.path)
        self.assertNotIn("duplicate_of", state["findings"][1])
        self.assertIn("2 / 100", self.path.read_text())

    def test_disproven_candidate_has_explicit_resolution(self):
        self.finding("E2E-001", "suspected")
        with self.assertRaisesRegex(SystemExit, "exclusion-reason"):
            self.cli("finding", "--id", "E2E-001", "--status", "excluded")
        self.cli("finding", "--id", "E2E-001", "--status", "excluded", "--exclusion-reason", "Documented behavior")
        self.assertIn("0 / 100", self.path.read_text())

    def test_header_tokens_are_removed_from_state_and_markdown(self):
        tokens = ["synthetic-bearer-token", "synthetic-basic-token", "synthetic-proxy-token"]
        payload = "\n".join(["Authorization: Bearer " + tokens[0],
                             "authorization = Basic " + tokens[1],
                             "Proxy-Authorization: Bearer " + tokens[2]])
        self.cli("event", "--message", payload)
        for artifact in (self.path, report.state_path(self.path)):
            for token in tokens:
                self.assertNotIn(token, artifact.read_text())

    def test_lane_recovery_and_replacement_history_survive_reload(self):
        self.cli("lane", "--lane", "3", "--status", "running", "--agent", "old-worker",
                 "--session", "old-session", "--health-url", "http://127.0.0.1:4230/health",
                 "--log-path", "fixture/server.log", "--namespace", "fixture-lane-03",
                 "--startup", "npm run dev", "--artifacts", "fixture/resources.json")
        self.cli("lane", "--lane", "3", "--agent", "replacement", "--session", "replacement-session")
        lane = report.load_state(self.path)["lanes"][0]
        self.assertEqual(["old-worker"], lane["previous_agents"])
        self.assertEqual(["old-session"], lane["previous_sessions"])
        self.assertEqual("fixture/resources.json", lane["artifacts"])
        self.assertEqual("fixture/server.log", lane["log_path"])

    def test_completion_requires_resolved_findings_and_checkpoints(self):
        self.resolved_lanes()
        with self.assertRaisesRegex(SystemExit, "checkpoint"):
            self.complete()
        self.checkpoints()
        self.finding("E2E-001", "reproduced", "--evidence", "fixture/trace")
        with self.assertRaisesRegex(SystemExit, "finding"):
            self.complete()
        self.cli("finding", "--id", "E2E-001", "--status", "verified", "--root-cause", "State reset",
                 "--fix", "Preserve state", "--regression", "Fails before, passes after", "--validation", "Journey passed")
        with self.assertRaisesRegex(SystemExit, "finding"):
            self.complete()
        self.cli("finding", "--id", "E2E-001", "--commit", "b" * 40)
        self.complete()
        self.assertEqual("completed", report.load_state(self.path)["status"])
        with self.assertRaisesRegex(SystemExit, "Reopen"):
            self.cli("lane", "--lane", "1", "--status", "running")

    def test_blocked_lane_and_checkpoint_prevent_completion(self):
        self.resolved_lanes()
        self.checkpoints()
        with self.assertRaisesRegex(SystemExit, "blocker"):
            self.cli("lane", "--lane", "1", "--status", "blocked")
        self.cli("lane", "--lane", "1", "--status", "blocked", "--blocker", "Required test tenant unavailable")
        with self.assertRaisesRegex(SystemExit, "Incomplete lanes"):
            self.complete()
        self.cli("lane", "--lane", "1", "--status", "completed")
        self.cli("checkpoint", "--name", "integration", "--outcome", "blocked", "--evidence", "Required DB unavailable")
        with self.assertRaisesRegex(SystemExit, "integration"):
            self.complete()

    def test_partial_save_recovers_markdown_from_json(self):
        real_write = report.atomic_write

        def interrupted_write(path, content):
            if path == self.path:
                raise OSError("Synthetic interruption after JSON write")
            real_write(path, content)

        with patch.object(report, "atomic_write", side_effect=interrupted_write):
            with self.assertRaises(OSError):
                self.cli("event", "--message", "Latest durable event")
        self.assertNotIn("Latest durable event", self.path.read_text())
        self.cli("render")
        self.assertIn("Latest durable event", self.path.read_text())

    def test_legacy_state_can_be_loaded_and_stopped(self):
        state = report.load_state(self.path)
        del state["checkpoints"]
        report.state_path(self.path).write_text(json.dumps(state))
        self.cli("status", "--status", "stopped", "--message", "Expansion budget spent")
        state = report.load_state(self.path)
        self.assertEqual({}, state["checkpoints"])
        self.assertEqual("stopped", state["status"])

    def test_audit_can_complete_with_diagnosed_unfixed_findings(self):
        self.path = self.path.with_name("audit.md")
        self.cli("init", "--repo", self.directory.name, "--run-id", "audit-fixture", "--mode", "audit")
        self.resolved_lanes()
        self.checkpoints()
        self.finding("E2E-001", "reproduced", "--evidence", "fixture/trace", "--root-cause", "State reset")
        self.finding("E2E-002", "duplicate", "--duplicate-of", "E2E-001")
        self.complete()
        state = report.load_state(self.path)
        self.assertEqual("audit", state["mode"])
        self.assertNotIn("fix", state["findings"][0])


class PortTests(unittest.TestCase):
    def plan(self):
        with patch.object(ports, "can_bind", return_value=True):
            return ports.create_plan("127.0.0.1", 4200, 3, 10)

    def test_replan_preserves_unstarted_lane_reservations(self):
        original = self.plan()
        snapshot = copy.deepcopy(original)
        reserved = {port for lane in original["lanes"] for port in lane["ports"].values()}
        with patch.object(ports, "can_bind", side_effect=lambda host, port: port != 4240):
            revised = ports.replan_lane(original, "02")
        self.assertEqual(snapshot, original)
        self.assertEqual(original["lanes"][0], revised["lanes"][0])
        self.assertEqual(original["lanes"][2], revised["lanes"][2])
        self.assertTrue(reserved.isdisjoint(revised["lanes"][1]["ports"].values()))
        self.assertEqual(4250, revised["lanes"][1]["ports"]["gateway"])

    def test_lane_check_ignores_other_running_lanes(self):
        plan = self.plan()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ports.json"
            ports.atomic_write_json(path, plan)
            args = ports.build_parser().parse_args(["check", "--plan", str(path), "--lane", "2"])
            with patch.object(ports, "can_bind", side_effect=lambda host, port: port >= 4220):
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    args.func(args)
            result = json.loads(output.getvalue())
            self.assertTrue(result["all_free"])
            self.assertEqual(["02"], [item["lane"] for item in result["lanes"]])

    def test_replan_rejects_unknown_lane_and_existing_output(self):
        with self.assertRaisesRegex(SystemExit, "absent"):
            ports.replan_lane(self.plan(), "99")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ports.json"
            ports.atomic_write_json(path, self.plan())
            before = path.read_bytes()
            with self.assertRaisesRegex(SystemExit, "overwrite"):
                ports.atomic_write_json(path, {})
            self.assertEqual(before, path.read_bytes())


if __name__ == "__main__":
    unittest.main()
