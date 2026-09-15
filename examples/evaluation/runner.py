"""Own one non-resumable evaluation, including cancellation and export cleanup."""
import asyncio
import hashlib
import time
from contextlib import AsyncExitStack
from pathlib import Path

from model_collaboration_engine import Engine

from .inputs import group_config, preflight, submission
from .reporting import accepts, group_report, safe_error, write_json

GROUPS = ("smoke", "single", "cascade")


def schedule(suite: dict) -> list[dict]:
    order = [("smoke", 0)]
    for index in range(len(suite["cases"])):
        groups = ("single", "cascade") if index % 2 == 0 else ("cascade", "single")
        order.extend((group, index) for group in groups)
    return [{"group": group, "case_index": index, "case_id": suite["cases"][index]["id"],
             "task_id": f"{group}-{index}", "status": "not_executed", "reason": "pending"}
            for group, index in order]


async def run_case(engine: Engine, suite: dict, row: dict) -> str | None:
    task = submission(suite, suite["cases"][row["case_index"]], row["group"], row["case_index"])
    row.update(status="running", submission=task, reason=None)
    started = time.perf_counter()
    try:
        row["result"] = await engine.run(task)
        row["status"] = row["result"]["status"]
    except asyncio.CancelledError:
        row["status"] = "cancelled"
        raise
    except RuntimeError as error:
        row.update(status="failed", error=safe_error(error))
    finally:
        row["elapsed_ms"] = (time.perf_counter() - started) * 1000
    metrics = await engine.metrics()
    if any(call["unknown_cost_attempts"] for call in metrics["calls"]):
        return "unknown_cost"
    if row.get("error") in {"budget", "storage", "configuration", "closed", "cleanup_timeout"}:
        return row["error"]
    if row["group"] == "smoke" and row["status"] != "completed":
        return "smoke_failed"
    return None


async def export_results(engines: dict, suite: dict, report: dict, output: Path) -> None:
    # No model calls occur here. Feedback cannot affect any paired task's routing snapshot.
    for row in report["runs"]:
        if row["status"] == "not_executed":
            row["reason"] = report["stop_reason"] or "not_started"
            continue
        engine = engines.get(row["group"])
        if engine is None:
            continue
        try:
            evaluations = await engine.evaluations(row["task_id"])
            row["evaluations"] = evaluations
            artifacts = [item["artifact"] for item in evaluations]
            row["artifact"] = max(artifacts, key=lambda a: a["version"]) if artifacts else None
            row["accepted"] = None
            if row["artifact"] is not None:
                row["accepted"] = accepts(row["artifact"]["text"],
                                          suite["cases"][row["case_index"]]["expected"])
                await engine.record_feedback({
                    "feedback_id": f"host-{row['task_id']}", "task_id": row["task_id"],
                    "artifact_id": row["artifact"]["artifact_id"], "kind": "business_acceptance",
                    "evaluator_version": suite["version"], "accepted": row["accepted"],
                    "reason": "Exact JSON key, type and value comparison against saved reference",
                })
        except RuntimeError as error:
            report["export_errors"].append({"task_id": row["task_id"], "kind": safe_error(error)})
    for group in GROUPS:
        metrics = None
        if group in engines:
            try:
                metrics = await engines[group].metrics()
                write_json(output / f"{group}.metrics.json", metrics)
                write_json(output / f"{group}.recovery.json", await engines[group].recovery_records())
            except (RuntimeError, OSError) as error:
                report["export_errors"].append({"group": group, "kind": safe_error(error)})
        report["groups"][group] = group_report(
            [r for r in report["runs"] if r["group"] == group], metrics)


async def finish(stack: AsyncExitStack, engines: dict, suite: dict,
                 report: dict, output: Path) -> None:
    try:
        await export_results(engines, suite, report, output)
    finally:
        try:
            await stack.aclose()
        except RuntimeError as error:
            report["export_errors"].append({"phase": "close", "kind": safe_error(error)})
        if report["export_errors"]:
            report["status"] = "incomplete"
        write_json(output / "report.json", report)


async def evaluate(config_path: Path, suite_path: Path, output: Path,
                   total_budget: int) -> dict:
    """Run paired cohorts; cancellation waits for evidence export and engine closure.

    Output must not exist. No resume/replay is performed. Prices are admission limits,
    not a guarantee against supplier overruns. Errors never export provider messages.
    """
    config, suite = preflight(config_path, suite_path, total_budget)
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    write_json(output / "manifest.json", {
        "config": config, "suite": suite, "total_admission_budget": total_budget,
        "planned_admission": (2 * len(suite["cases"]) + 1) * suite["budget"],
        "config_sha256": hashlib.sha256(config_path.read_bytes()).hexdigest(),
        "suite_sha256": hashlib.sha256(suite_path.read_bytes()).hexdigest(),
    })
    report = {"status": "running", "stop_reason": None, "evaluator_version": suite["version"],
              "runs": schedule(suite), "groups": {}, "export_errors": [],
              "caution": "Smoke is separate. Completion is not business acceptance. Unknown costs "
                         "are not zero. Task elapsed time differs from mean call latency. "
                         "This small trial does not establish causal or statistically significant gains."}
    write_json(output / "report.json", report)
    stack, engines = AsyncExitStack(), {}
    cancelled = False
    try:
        for group in GROUPS:
            engines[group] = await stack.enter_async_context(
                await Engine.open(group_config(config, output, group)))
        for row in report["runs"]:
            reason = await run_case(engines[row["group"]], suite, row)
            write_json(output / "report.json", report)
            if reason:
                report.update(status="stopped", stop_reason=reason)
                break
        else:
            report["status"] = "completed"
    except asyncio.CancelledError:
        cancelled = True
        report.update(status="cancelled", stop_reason="cancelled")
    except Exception as error:
        # The example is an outer orchestration boundary; preserve partial output on any failure.
        report.update(status="incomplete", stop_reason=safe_error(error))
    finally:
        cleanup = asyncio.create_task(finish(stack, engines, suite, report, output))
        while not cleanup.done():
            try:
                await asyncio.shield(cleanup)
            except asyncio.CancelledError:
                cancelled = True
                report.update(status="cancelled", stop_reason="cancelled")
        cleanup.result()
    if cancelled:
        raise asyncio.CancelledError
    return report
