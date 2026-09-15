"""Compare exported metrics from controlled cohorts; never dispatch or replay tasks.

Usage: python examples/compare_metrics.py baseline.json candidate.json
Export each JSON from await engine.metrics(), using one database per cohort.
"""
import argparse
import json
from pathlib import Path
from typing import Any


def summarize(snapshot: dict[str, Any]) -> dict[str, Any]:
    calls = snapshot["calls"]
    tasks = snapshot["tasks"]
    terminal = tasks["total"] - tasks["running"]
    latency_samples = sum(c["latency_samples"] for c in calls)
    latency_total = sum(
        c["mean_latency_ms"] * c["latency_samples"]
        for c in calls if c["latency_samples"]
    )
    return {
        "feedback_revision": snapshot["revision"],
        "tasks": tasks,
        "execution_completion_rate": tasks["completed"] / terminal if terminal else None,
        "human_required_rate": tasks["human_required"] / terminal if terminal else None,
        "known_cost": sum(c["known_cost"] for c in calls),
        "unresolved_reserved": sum(c["unresolved_reserved"] for c in calls),
        "attempts": sum(c["attempts"] for c in calls),
        "unknown_cost_attempts": sum(c["unknown_cost_attempts"] for c in calls),
        "unknown_status": sum(c["unknown_status"] for c in calls),
        "latency_samples": latency_samples,
        "mean_attempt_latency_ms": latency_total / latency_samples if latency_samples else None,
        "planner_attempts": sum(c["attempts"] for c in calls if c["node"] == "planner"),
        "planner_known_cost": sum(c["known_cost"] for c in calls if c["node"] == "planner"),
        # Preserve versioned keys. A blended pass rate would silently mix evaluators.
        "quality": [
            row | {"acceptance_rate": row["accepted"] / row["samples"] if row["samples"] else None}
            for row in snapshot["quality"]
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    args = parser.parse_args()
    report = {
        "baseline": summarize(json.loads(args.baseline.read_text(encoding="utf-8"))),
        "candidate": summarize(json.loads(args.candidate.read_text(encoding="utf-8"))),
        "caution": (
            "Use the same task set, acceptance definition and workload conditions. "
            "Completion is not business acceptance; unknown costs are not zero. "
            "Mean attempt latency is not end-to-end task latency. No causal benefit is inferred."
        ),
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
