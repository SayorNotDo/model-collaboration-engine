"""Four bounded live protocol probes; synthetic fixtures do not measure quality gains."""
import argparse
import asyncio
import copy
import json
import os
import sqlite3
import time
from contextlib import closing
from pathlib import Path
from typing import Any

from evaluation.inputs import read_document
from evaluation.reporting import safe_error, write_json

from model_collaboration_engine import Engine, parse_config

SCENARIOS = ("cascade", "generator_critic", "planner", "tools")


def submission(name: str, budget: int, output_tokens: int) -> dict:
    """Construct synthetic, role-aware fixtures with shared finite call limits."""
    task = {
        "task_id": name, "task_type": "reasoning", "strategy": "single",
        "goal": 'Return only {"answer":42}.', "evidence": [], "tools": [],
        "planning": {"mode": "disabled", "max_cost": 0, "timeout_ms": 60000},
        "acceptance": {"version": "collaboration-probe-v1", "nonempty": True,
                       "json_object": True, "required_substrings": []},
        "selection": {"version": "collaboration-probe-selection-v1", "min_quality": 0.0,
                      "target_quality": 0.9, "above_target_factor": 0.1,
                      "cost_reference": 10000, "latency_reference_ms": 5000,
                      "min_upgrade_gain": 0.05},
        "constraints": {"allowed_models": [], "allowed_providers": [], "allowed_regions": [],
                        "local_only": False, "required_capabilities": ["text", "json"],
                        "preferred_capabilities": [], "different_critic": False},
        "budget": budget, "max_call_cost": budget // 4, "max_calls": 4,
        "max_rounds": 2, "max_attempts": 1, "output_tokens": output_tokens,
        "deadline_ms": int(time.time() * 1000) + 120000, "finalization_ms": 2000,
    }
    if name == "cascade":
        task["strategy"] = "cascade"
        task["acceptance"]["required_substrings"] = ["PROBE_COMPLETE"]
        task["goal"] = (
            "Synthetic orchestration test: when snapshot version is 1 intentionally return "
            '{"answer":42} without the acceptance marker to exercise rejection. '
            'When version is greater than 1, return {"answer":42,"marker":"PROBE_COMPLETE"}. '
            "This deliberate first failure tests protocol, not real model quality.")
    elif name == "generator_critic":
        task["strategy"] = "generator_critic"
        task["constraints"]["different_critic"] = True
        task["goal"] = (
            "Synthetic revision test. Final answer must equal 19+23=42. "
            "Only role=generator at version=1 should intentionally draft answer=41. "
            "Role=generator at later versions must fix to answer=42. "
            "Role=critic must independently check arithmetic in prior_artifact, always "
            "return revise with a concrete defect for answer=41, even though it was "
            "deliberately drafted. Return pass only for answer=42. Generator outputs JSON.")
    elif name == "planner":
        task["planning"] = {"mode": "required", "max_cost": budget // 4,
                            "max_calls": 1, "timeout_ms": 60000}
        task["max_calls"] = 2
    elif name == "tools":
        task["task_type"] = "tool_execution"
        task["max_calls"] = 3
        task["constraints"]["required_capabilities"].append("tools")
        task["goal"] = (
            'Call lookup exactly once with {"key":"probe"}. Return a JSON object with '
            'answer equal to the tool output value. Do not invent the value or call other tools.')
        task["tools"] = [{"name": "lookup", "description": "Read the fixed probe value",
                          "parameters": {"type": "object", "properties": {
                              "key": {"type": "string", "enum": ["probe"]}},
                              "required": ["key"], "additionalProperties": False},
                          "max_cost": 0}]
    return task


def read_evidence(path: Path, task_id: str) -> dict:
    """Read one consistent current-schema snapshot without opening a writable connection."""
    with closing(sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True)) as database:
        database.row_factory = sqlite3.Row
        database.execute("BEGIN")
        task = database.execute("SELECT * FROM tasks WHERE id=?", (task_id,)).fetchone()
        if task is None:
            raise ValueError("missing persisted task")
        ledger = {key: task[key] for key in ("status", "total", "settled", "reserved", "calls")}
        attempts = []
        for row in database.execute("SELECT * FROM attempts WHERE task=? ORDER BY rowid", (task_id,)):
            item = dict(row)
            item["metadata"] = json.loads(item["metadata"])
            outcome = json.loads(item["outcome"]) if item["outcome"] else {}
            # Arbitrary supplier error messages are deliberately excluded from the JSON export.
            item["outcome"] = {key: outcome[key] for key in
                               ("request_id", "usage", "call_metrics", "kind") if key in outcome}
            attempts.append(item)
        evaluations = [json.loads(row[0]) for row in database.execute(
            "SELECT record FROM evaluations WHERE task=? ORDER BY rowid", (task_id,))]
        return {"ledger": ledger, "plan": json.loads(task["plan"]),
                "attempts": attempts, "evaluations": evaluations}


def assess(name: str, evidence: dict, tool_calls: list[dict]) -> dict:
    """Only claim exercised paths with persisted attempt and evaluation evidence."""
    attempts, evaluations = evidence["attempts"], evidence["evaluations"]
    succeeded = [a for a in attempts if a["state"] == "settled" and
                 a["outcome"].get("call_metrics", {}).get("status") == "succeeded"]
    models = [a for a in succeeded if "snapshot" in a["metadata"]]
    roles = [a["metadata"]["snapshot"]["role"] for a in models]
    revisions = [e for e in evaluations if e.get("critic", {}) and
                 e["critic"]["evaluation"]["status"] == "revise"]
    observed = False
    if name == "cascade":
        ids = {a["metadata"]["route"]["model_id"] for a in models}
        observed = len(ids) >= 2 and any(
            e["deterministic"]["status"] == "revise" for e in evaluations)
    elif name == "generator_critic":
        observed = bool(revisions) and any(
            a["metadata"]["snapshot"]["role"] == "generator" and
            a["metadata"]["snapshot"]["version"] > 1 and
            a["metadata"]["snapshot"]["feedback"] for a in models)
    elif name == "planner":
        # Planner reservation metadata uses role, not an execution snapshot.
        observed = any(a["metadata"].get("role") == "planner" for a in succeeded)
        observed = observed and bool(evidence["plan"].get("effective_plan")) and bool(models)
    elif name == "tools":
        settled_tools = [a for a in succeeded if a["metadata"].get("kind") == "tool"]
        observed = len(tool_calls) == len(settled_tools) == 1 and len(models) >= 2
    accepted = None
    if evaluations:
        try:
            artifact = max(evaluations, key=lambda e: e["artifact"]["version"])["artifact"]
            actual = json.loads(artifact["text"], object_pairs_hook=unique_fields)
            expected = "lookup-confirmed-42" if name == "tools" else 42
            accepted = (isinstance(actual, dict) and type(actual.get("answer")) is type(expected)
                        and actual["answer"] == expected)
        except (ValueError, KeyError, TypeError):
            accepted = False
    return {"path_observed": observed, "roles": roles,
            "final_artifact_accepted": accepted,
            "request_ids": [a["outcome"]["request_id"] for a in attempts
                            if a["outcome"].get("request_id")],
            "completed": evidence["ledger"]["status"] == "completed"}


def unique_fields(pairs: list[tuple[str, Any]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON field")
        result[key] = value
    return result


async def run_one(engine: Any, row: dict, database: Path, budget: int,
                  output_tokens: int) -> str | None:
    calls: list[dict] = []

    async def lookup(request: dict) -> dict:
        if (request["task_id"] != "tools" or request["call"]["name"] != "lookup"
                or request["call"]["arguments"] != {"key": "probe"}
                or request["max_cost"] != 0 or calls):
            raise ValueError("probe tool request denied")
        calls.append({"execution_id": request["execution_id"], "arguments": {"key": "probe"}})
        return {"output": '{"value":"lookup-confirmed-42"}', "actual_cost": 0}

    row.update(status="running", submission=submission(row["scenario"], budget, output_tokens))
    try:
        row["result"] = await engine.run(row["submission"], tools={"lookup": lookup})
        row["status"] = row["result"]["status"]
    except RuntimeError as error:
        row.update(status="failed", error=safe_error(error))
    finally:
        row["tool_calls"] = calls
    evidence = await asyncio.to_thread(read_evidence, database, row["scenario"])
    row["evidence"] = evidence
    row["assessment"] = assess(row["scenario"], evidence, calls)
    if any(a["cost"] is None or a["state"] != "settled" for a in evidence["attempts"]):
        return "unknown_cost"
    if evidence["ledger"]["settled"] > budget or any(
            a["cost"] > a["amount"] for a in evidence["attempts"]):
        return "overrun"
    if row.get("error") in {"budget", "storage", "configuration", "closed", "cleanup_timeout"}:
        return row["error"]
    return None


async def finish_one(engine: Any, row: dict, database: Path) -> None:
    try:
        await engine.close()
    finally:
        if "evidence" not in row:
            row["evidence"] = await asyncio.to_thread(read_evidence, database, row["scenario"])
            row["status"] = row["evidence"]["ledger"]["status"]


def validation_passed(report: dict) -> bool:
    """Distinguish completing the schedule from validating every requested path."""
    return len(report["runs"]) == len(SCENARIOS) and all(
        row["status"] == "completed"
        and row.get("assessment", {}).get("path_observed") is True
        and row.get("assessment", {}).get("final_artifact_accepted") is True
        for row in report["runs"]
    )


async def probe(config_path: Path, output: Path, total_budget: int,
                output_tokens: int = 4096) -> dict:
    """Run at most four fresh tasks; repeated cancellation waits for engine close."""
    if type(total_budget) is not int or not 16 <= total_budget <= 2**63 - 1:
        raise ValueError("total budget must be integer microcredits >=16")
    if not 4096 <= output_tokens <= 16384:
        raise ValueError("output tokens must be between 4096 and 16384")
    config = read_document(config_path)
    parse_config(config, base_dir=config_path.parent)
    models = [m for p in config["providers"] for m in p["models"]]
    if not config.get("routing", {}).get("planner_models"):
        raise ValueError("requires an explicit nonempty routing.planner_models pool")
    if len(models) < 2 or not any("tools" in m["capabilities"] for m in models):
        raise ValueError("requires two candidates and a tools-capable candidate")
    for provider in config["providers"]:
        if not os.environ.get(provider["auth"]["env"], "").strip():
            raise ValueError("missing explicitly configured credential environment variable")
    if any(not m.get("price_version") or "example" in m["price_version"] for m in models):
        raise ValueError("explicit reviewed price_version required")
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"status": "running", "stop_reason": None, "total_admission_budget": total_budget,
              "caution": "Synthetic protocol conformance, not evidence of natural quality gains.",
              "runs": [{"scenario": n, "status": "not_executed"} for n in SCENARIOS]}
    write_json(output / "manifest.json", {"config": config, "total_budget": total_budget,
                                         "output_tokens": output_tokens})
    cancelled = False
    try:
        for row in report["runs"]:
            document = copy.deepcopy(config)
            database = output / (row["scenario"] + ".db")
            document["storage"]["database_path"] = str(database)
            engine = await Engine.open(document)
            try:
                reason = await run_one(engine, row, database, total_budget // 4, output_tokens)
            finally:
                cleanup = asyncio.create_task(finish_one(engine, row, database))
                while not cleanup.done():
                    try:
                        await asyncio.shield(cleanup)
                    except asyncio.CancelledError:
                        cancelled = True
                cleanup.result()
            if cancelled:
                raise asyncio.CancelledError
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
        # This is the outer CLI boundary; do not expose arbitrary provider errors.
        kind = "storage" if isinstance(error, sqlite3.Error) else safe_error(error)
        report.update(status="incomplete", stop_reason=kind)
    finally:
        report["validation_passed"] = validation_passed(report)
        write_json(output / "report.json", report)
    if cancelled:
        raise asyncio.CancelledError
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--total-budget", type=int, required=True)
    parser.add_argument("--output-tokens", type=int, default=4096)
    args = parser.parse_args()
    result = asyncio.run(probe(args.config, args.output, args.total_budget, args.output_tokens))
    print(json.dumps({key: result[key] for key in
                      ("status", "stop_reason", "validation_passed")}))
    if result["status"] != "completed" or not result["validation_passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
