"""Measure full-scan metrics on disposable synthetic data; never dispatch models.

Run from the repository root: python examples/benchmark_metrics.py --rows 100 1000 10000
This benchmarks task/attempt aggregation with one model group and no quality feedback.
It is not an end-to-end submission benchmark or a production latency guarantee.
"""
import argparse
import asyncio
import json
import platform
import sqlite3
import statistics
import tempfile
import time
from contextlib import closing
from pathlib import Path

from model_collaboration_engine import Engine


def seed(path: Path, count: int) -> None:
    """Populate only a newly initialized disposable schema, never a user database."""
    metadata = json.dumps({"route": {"model_id": "synthetic", "model_version": "1",
                                     "node_id": "invoke"}})
    outcome = json.dumps({"call_metrics": {"status": "succeeded", "latency_ms": 10}})
    with closing(sqlite3.connect(path)) as db, db:
        db.execute("PRAGMA foreign_keys=ON")
        db.executemany(
            "INSERT INTO tasks(id,spec,config_hash,plan,status,total,settled,calls,checkpoint) "
            "VALUES (?,'{}','synthetic','{}','completed',10,3,1,'{}')",
            ((str(index),) for index in range(count)),
        )
        db.executemany(
            "INSERT INTO attempts(id,task,amount,cost,state,metadata,outcome) "
            "VALUES (?,?,3,3,'settled',?,?)",
            ((str(index), str(index), metadata, outcome) for index in range(count)),
        )


async def benchmark(sizes: list[int], repeats: int = 5) -> dict:
    """Return warmed query timings plus verified metrics for each bounded dataset."""
    if (not sizes or len(sizes) > 10
            or any(type(n) is not int or not 1 <= n <= 1_000_000 for n in sizes)
            or type(repeats) is not int or not 1 <= repeats <= 100):
        raise ValueError("require 1..10 sizes (1..1000000 rows) and 1..100 repeats")
    config = json.loads((Path(__file__).parent / "config.json").read_text(encoding="utf-8"))
    report = {"platform": platform.platform(), "python": platform.python_version(),
              "shape": "one completed task per settled attempt; one model group; no feedback",
              "warmup_queries": 1, "samples": []}
    for count in sizes:
        with tempfile.TemporaryDirectory(prefix="engine-metrics-") as temporary:
            path = Path(temporary) / "synthetic.db"
            config["database_path"] = str(path)
            async with await Engine.open(config):
                pass
            seed(path, count)
            async with await Engine.open(config) as engine:
                await engine.metrics()
                elapsed = []
                for _ in range(repeats):
                    start = time.perf_counter()
                    metrics = await engine.metrics()
                    elapsed.append((time.perf_counter() - start) * 1000)
                calls = metrics["calls"]
                attempts = sum(item["attempts"] for item in calls)
                known_cost = sum(item["known_cost"] for item in calls)
                unknown = sum(item["unknown_cost_attempts"] for item in calls)
                if (metrics["tasks"]["total"] != count or attempts != count
                        or known_cost != count * 3 or unknown != 0):
                    raise RuntimeError("synthetic metrics failed accounting/count checks")
                report["samples"].append({
                    "tasks": count, "attempts": attempts, "known_cost": known_cost,
                    "unknown_cost_attempts": unknown, "elapsed_ms": elapsed,
                    "median_ms": statistics.median(elapsed), "max_ms": max(elapsed),
                })
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rows", type=int, nargs="+", default=[100, 1000, 10000])
    parser.add_argument("--repeats", type=int, default=5)
    args = parser.parse_args()
    print(json.dumps(asyncio.run(benchmark(args.rows, args.repeats)), indent=2))


if __name__ == "__main__":
    main()
