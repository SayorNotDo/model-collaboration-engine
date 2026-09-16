import asyncio

from examples.benchmark_metrics import benchmark


def test_synthetic_metrics_preserve_counts_and_known_costs():
    report = asyncio.run(benchmark([3, 9], repeats=2))
    assert [row["attempts"] for row in report["samples"]] == [3, 9]
    assert [row["known_cost"] for row in report["samples"]] == [9, 27]
    assert all(len(row["elapsed_ms"]) == 2 for row in report["samples"])
    assert all(row["unknown_cost_attempts"] == 0 for row in report["samples"])
