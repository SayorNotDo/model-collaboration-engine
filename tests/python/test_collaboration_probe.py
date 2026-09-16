"""Offline checks for probe admission, evidence claims and fail-closed sequencing."""
import asyncio
import copy
import importlib
import json
import sqlite3
from pathlib import Path

import pytest


@pytest.fixture
def probe(monkeypatch):
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[2] / "examples"))
    return importlib.import_module("collaboration_probe")


def evidence(cost=10, amount=20, state="settled"):
    return {"ledger": {"status": "completed", "settled": cost or 0},
            "plan": {"effective_plan": {}}, "evaluations": [],
            "attempts": [{"cost": cost, "amount": amount, "state": state,
                          "metadata": {}, "outcome": {"request_id": "provider-response-1",
                                                       "call_metrics": {"status": "succeeded"}}}]}


@pytest.mark.parametrize(("cost", "amount", "state", "reason"), [
    (None, 0, "unresolved", "unknown_cost"),
    (21, 20, "settled", "overrun"),
    (10, 20, "settled", None),
])
def test_unknown_zero_reservation_and_overrun_stop(probe, monkeypatch, cost, amount, state, reason):
    class Engine:
        async def run(self, task, tools):
            return {"status": "completed"}

    monkeypatch.setattr(probe, "read_evidence", lambda *_: evidence(cost, amount, state))
    row = {"scenario": "cascade"}
    assert asyncio.run(probe.run_one(Engine(), row, Path("unused"), 1000, 4096)) == reason
    assert row["assessment"]["path_observed"] is False
    assert row["assessment"]["request_ids"] == ["provider-response-1"]


def test_revision_requires_critic_rejection_and_feedback(probe):
    record = evidence()
    record["evaluations"] = [{"critic": {"evaluation": {"status": "revise"}}}]
    record["attempts"][0]["metadata"] = {
        "snapshot": {"role": "generator", "version": 2, "feedback": []}}
    assert not probe.assess("generator_critic", record, [])["path_observed"]
    record["attempts"][0]["metadata"]["snapshot"]["feedback"] = ["wrong arithmetic"]
    assert probe.assess("generator_critic", record, [])["path_observed"]


@pytest.mark.parametrize("storage_failure", [False, True])
def test_probe_stops_before_next_task_and_closes(probe, monkeypatch, tmp_path, storage_failure):
    config = {"providers": [{"auth": {"env": "PROBE_KEY"}, "models": [
        {"capabilities": ["tools"], "price_version": "reviewed"},
        {"capabilities": [], "price_version": "reviewed"}]}], "storage": {},
        "routing": {"planner_models": ["first"]}}
    path = tmp_path / "config.json"
    path.write_text(json.dumps(config))
    monkeypatch.setenv("PROBE_KEY", "local-test-only")
    monkeypatch.setattr(probe, "parse_config", lambda *_args, **_kwargs: None)
    def read(_path, _task):
        if storage_failure:
            raise sqlite3.OperationalError("private database failure details")
        return evidence(None, 0, "unresolved")

    monkeypatch.setattr(probe, "read_evidence", read)
    opened, closed = [], []

    class Engine:
        @staticmethod
        async def open(document):
            opened.append(copy.deepcopy(document))
            return Engine()

        async def run(self, task, tools):
            return {"status": "completed"}

        async def close(self):
            closed.append(True)

    monkeypatch.setattr(probe, "Engine", Engine)
    output = tmp_path / "out"
    report = asyncio.run(probe.probe(path, output, 4000))
    assert len(opened) == len(closed) == 1
    reason = "storage" if storage_failure else "unknown_cost"
    assert report["stop_reason"] == reason
    assert [r["status"] for r in report["runs"]] == ["completed"] + ["not_executed"] * 3
    assert json.loads((output / "report.json").read_text())["stop_reason"] == reason


def test_tool_rejects_extra_arguments(probe, monkeypatch):
    class Engine:
        async def run(self, task, tools):
            with pytest.raises(ValueError, match="denied"):
                await tools["lookup"]({"task_id": "tools", "max_cost": 0,
                                       "call": {"name": "lookup", "arguments": {
                                           "key": "probe", "unsafe": True}}})
            return {"status": "completed"}

    monkeypatch.setattr(probe, "read_evidence", lambda *_: evidence())
    row = {"scenario": "tools"}
    asyncio.run(probe.run_one(Engine(), row, Path("unused"), 1000, 4096))
    assert row["tool_calls"] == []


def test_submission_limits_and_forced_planning(probe):
    tasks = [probe.submission(name, 1000000, 4096) for name in probe.SCENARIOS]
    assert sum(t["budget"] for t in tasks) == 4000000
    assert all(t["max_calls"] <= 4 and t["max_attempts"] == 1 for t in tasks)
    assert tasks[2]["planning"]["mode"] == "required"
    assert tasks[2]["planning"]["max_cost"] == 250000


def test_reads_persisted_snapshot_and_sanitizes_supplier_error(probe, tmp_path):
    database = tmp_path / "evidence.db"
    with sqlite3.connect(database) as connection:
        connection.executescript(
            "CREATE TABLE tasks(id,status,total,settled,reserved,calls,plan);"
            "CREATE TABLE attempts(id,task,amount,cost,state,metadata,outcome);"
            "CREATE TABLE evaluations(task,record);")
        connection.execute("INSERT INTO tasks VALUES(?,?,?,?,?,?,?)",
                           ("planner", "completed", 1000, 10, 0, 1,
                            json.dumps({"effective_plan": {"strategy": "single"}})))
        connection.execute("INSERT INTO attempts VALUES(?,?,?,?,?,?,?)", (
            "call", "planner", 20, 10, "settled", json.dumps({"role": "planner"}),
            json.dumps({"request_id": "response-1", "error": "private-provider-message",
                        "call_metrics": {"status": "succeeded"}})))
    connection.close()
    result = probe.read_evidence(database, "planner")
    assert result["attempts"][0]["metadata"]["role"] == "planner"
    assert result["attempts"][0]["outcome"]["request_id"] == "response-1"
    assert "private-provider-message" not in json.dumps(result)
    # Windows refuses this when the helper leaks its SQLite connection.
    database.unlink()


def test_failed_reservation_is_not_execution_evidence(probe):
    record = evidence()
    record["attempts"][0]["metadata"] = {"role": "planner"}
    record["attempts"][0]["outcome"]["call_metrics"]["status"] = "failed"
    assert not probe.assess("planner", record, [])["path_observed"]


@pytest.mark.parametrize("text", ['{"answer":41}', '{"answer":41,"answer":42}'])
def test_final_answer_check_is_independent_of_completion(probe, text):
    record = evidence()
    record["evaluations"] = [{"artifact": {"version": 1, "text": text},
                              "critic": None}]
    assessment = probe.assess("generator_critic", record, [])
    assert assessment["completed"]
    assert not assessment["final_artifact_accepted"]


def test_repeated_cancellation_waits_for_close_and_exports(probe, monkeypatch, tmp_path):
    config = {"providers": [{"auth": {"env": "PROBE_KEY"}, "models": [
        {"capabilities": ["tools"], "price_version": "reviewed"},
        {"capabilities": [], "price_version": "reviewed"}]}], "storage": {},
        "routing": {"planner_models": ["first"]}}
    monkeypatch.setenv("PROBE_KEY", "local-test-only")
    monkeypatch.setattr(probe, "read_document", lambda *_: config)
    monkeypatch.setattr(probe, "parse_config", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(probe, "read_evidence", lambda *_: evidence())

    async def exercise():
        entered, closing, release, closed = (asyncio.Event() for _ in range(4))

        class Engine:
            @staticmethod
            async def open(_):
                return Engine()

            async def run(self, task, tools):
                entered.set()
                await asyncio.Event().wait()

            async def close(self):
                closing.set()
                await release.wait()
                closed.set()

        monkeypatch.setattr(probe, "Engine", Engine)
        task = asyncio.create_task(probe.probe(tmp_path / "unused", tmp_path / "out", 4000))
        await asyncio.wait_for(entered.wait(), 2)
        task.cancel()
        await asyncio.wait_for(closing.wait(), 2)
        task.cancel()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 2)
        assert closed.is_set()

    asyncio.run(exercise())
    report = json.loads((tmp_path / "out" / "report.json").read_text())
    assert report["status"] == "cancelled"
    assert "evidence" in report["runs"][0]
    assert all(row["status"] == "not_executed" for row in report["runs"][1:])


@pytest.mark.parametrize(("status", "observed", "accepted", "expected"), [
    ("completed", True, True, True),
    ("failed", True, True, False),
    ("completed", False, True, False),
    ("completed", True, None, False),
    ("completed", True, False, False),
])
def test_validation_requires_every_path_and_final_answer(probe, status, observed, accepted, expected):
    row = {"status": status, "assessment": {
        "path_observed": observed, "final_artifact_accepted": accepted}}
    assert probe.validation_passed({"runs": [row] * 4}) is expected
    assert not probe.validation_passed({"runs": [row] * 3})
