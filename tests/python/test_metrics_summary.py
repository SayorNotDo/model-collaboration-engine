"""Offline summaries keep unknown costs and evaluator strata visible."""
import importlib.util
from pathlib import Path

path = Path(__file__).resolve().parents[2] / "examples" / "compare_metrics.py"
spec = importlib.util.spec_from_file_location("compare_metrics", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def test_summary_preserves_denominators_and_unknown_costs():
    snapshot = {
        "revision": 2,
        "tasks": {"total": 4, "running": 1, "completed": 2, "human_required": 1},
        "calls": [
            {"node": "planner", "known_cost": 10, "unresolved_reserved": 20, "attempts": 3,
             "unknown_status": 1, "unknown_cost_attempts": 1, "latency_samples": 2, "mean_latency_ms": 50},
            {"node": "invoke", "known_cost": 30, "unresolved_reserved": 0, "attempts": 1,
             "unknown_status": 0, "unknown_cost_attempts": 0, "latency_samples": 1, "mean_latency_ms": 20},
        ],
        "quality": [
            {"key": {"evaluator_version": "1"}, "kind": "business_acceptance",
             "accepted": 1, "samples": 2},
            {"key": {"evaluator_version": "2"}, "kind": "business_acceptance",
             "accepted": 1, "samples": 1},
        ],
    }
    summary = module.summarize(snapshot)
    assert summary["execution_completion_rate"] == 2 / 3
    assert summary["human_required_rate"] == 1 / 3
    assert summary["mean_attempt_latency_ms"] == 40
    assert summary["known_cost"] == 40
    assert summary["unresolved_reserved"] == 20
    assert summary["planner_attempts"] == 3
    assert [row["acceptance_rate"] for row in summary["quality"]] == [0.5, 1.0]
