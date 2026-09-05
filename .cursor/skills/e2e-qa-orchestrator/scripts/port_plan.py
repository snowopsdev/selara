#!/usr/bin/env python3
"""Plan free loopback port blocks for isolated E2E QA lanes."""

from __future__ import annotations

import argparse
import copy
import json
import os
import socket
import tempfile
from pathlib import Path
from typing import Any


ROLE_OFFSETS = {"gateway": 0, "app": 1, "callback": 2}


def can_bind(host: str, port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            probe.bind((host, port))
        except OSError:
            return False
    return True


def block_is_free(host: str, gateway: int, used: set[int]) -> bool:
    ports = {gateway + offset for offset in ROLE_OFFSETS.values()}
    return not (ports & used) and all(can_bind(host, port) for port in ports)


def create_plan(host: str, base: int, lanes: int, stride: int) -> dict[str, Any]:
    if not 1 <= lanes <= 100:
        raise SystemExit("--lanes must be between 1 and 100")
    if stride < 3:
        raise SystemExit("--stride must be at least 3")
    if not 1024 <= base <= 65500:
        raise SystemExit("--base must be between 1024 and 65500")

    used: set[int] = set()
    assignments: list[dict[str, Any]] = []
    for lane in range(1, lanes + 1):
        candidate = base + lane * stride
        while candidate + max(ROLE_OFFSETS.values()) <= 65535:
            if block_is_free(host, candidate, used):
                ports = {role: candidate + offset for role, offset in ROLE_OFFSETS.items()}
                used.update(ports.values())
                assignments.append({"lane": f"{lane:02d}", "ports": ports})
                break
            candidate += stride
        else:
            raise SystemExit(f"No free port block available for lane {lane:02d}")

    return {
        "host": host,
        "requested_base": base,
        "stride": stride,
        "lanes": assignments,
        "note": "Availability is a point-in-time probe; recheck immediately before process startup.",
    }


def atomic_write_json(path: Path, payload: dict[str, Any]) -> None:
    path = path.expanduser().resolve()
    if path.exists():
        raise SystemExit(f"Refusing to overwrite existing port plan: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as handle:
        json.dump(payload, handle, indent=2, sort_keys=True)
        handle.write("\n")
        temporary = Path(handle.name)
    os.replace(temporary, path)


def plan_command(args: argparse.Namespace) -> None:
    payload = create_plan(args.host, args.base, args.lanes, args.stride)
    if args.output:
        output = Path(args.output)
        atomic_write_json(output, payload)
        print(output.expanduser().resolve())
    else:
        print(json.dumps(payload, indent=2, sort_keys=True))


def replan_lane(payload: dict[str, Any], lane_id: str) -> dict[str, Any]:
    """Replace one block while preserving every other lane's reservation."""
    updated = copy.deepcopy(payload)
    assignment = next((item for item in updated["lanes"] if item["lane"] == lane_id), None)
    if assignment is None:
        raise SystemExit(f"Lane {lane_id} is absent from the plan")
    used = {int(port) for item in updated["lanes"] for port in item["ports"].values()}
    candidate = int(updated["requested_base"]) + int(lane_id) * int(updated["stride"])
    while candidate + max(ROLE_OFFSETS.values()) <= 65535:
        if block_is_free(updated["host"], candidate, used):
            assignment["ports"] = {role: candidate + offset for role, offset in ROLE_OFFSETS.items()}
            return updated
        candidate += int(updated["stride"])
    raise SystemExit(f"No free port block available for lane {lane_id}")


def replan_command(args: argparse.Namespace) -> None:
    source = Path(args.plan).expanduser().resolve()
    with source.open(encoding="utf-8") as handle:
        payload = json.load(handle)
    updated = replan_lane(payload, f"{args.lane:02d}")
    atomic_write_json(Path(args.output), updated)
    print(Path(args.output).expanduser().resolve())


def check_command(args: argparse.Namespace) -> None:
    plan_path = Path(args.plan).expanduser().resolve()
    with plan_path.open(encoding="utf-8") as handle:
        payload = json.load(handle)
    host = payload["host"]
    results = []
    for assignment in payload["lanes"]:
        if args.lane is not None and assignment["lane"] != f"{args.lane:02d}":
            continue
        ports = assignment["ports"]
        availability = {role: can_bind(host, int(port)) for role, port in ports.items()}
        results.append({"lane": assignment["lane"], "available": availability, "all_free": all(availability.values())})
    if not results:
        raise SystemExit("Requested lane is absent from the plan")
    result = {"plan": str(plan_path), "lanes": results, "all_free": all(item["all_free"] for item in results)}
    print(json.dumps(result, indent=2, sort_keys=True))
    if not result["all_free"]:
        raise SystemExit(2)


def self_test_command(_: argparse.Namespace) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as occupied:
        occupied.bind(("127.0.0.1", 0))
        occupied_port = occupied.getsockname()[1]
        base = occupied_port - 10
        plan = create_plan("127.0.0.1", base, lanes=3, stride=10)
        assigned = [port for lane in plan["lanes"] for port in lane["ports"].values()]
        assert occupied_port not in assigned
        assert len(assigned) == len(set(assigned)) == 9
        assert all(port >= 1024 for port in assigned)
    print("port_plan.py self-test passed")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    plan_parser = subparsers.add_parser("plan", help="create a free lane-port plan")
    plan_parser.add_argument("--host", default="127.0.0.1")
    plan_parser.add_argument("--base", type=int, default=4200)
    plan_parser.add_argument("--lanes", type=int, default=12)
    plan_parser.add_argument("--stride", type=int, default=10)
    plan_parser.add_argument("--output")
    plan_parser.set_defaults(func=plan_command)

    check_parser = subparsers.add_parser("check", help="recheck a saved plan")
    check_parser.add_argument("--plan", required=True)
    check_parser.add_argument("--lane", type=int, help="check only a lane that has not started yet")
    check_parser.set_defaults(func=check_command)

    replan_parser = subparsers.add_parser("replan", help="replace one lane's ports in a new plan revision")
    replan_parser.add_argument("--plan", required=True)
    replan_parser.add_argument("--lane", type=int, required=True)
    replan_parser.add_argument("--output", required=True)
    replan_parser.set_defaults(func=replan_command)

    test_parser = subparsers.add_parser("self-test", help="run an isolated smoke test")
    test_parser.set_defaults(func=self_test_command)
    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
