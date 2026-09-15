"""Feedback through both HTTP endpoints and the installed native boundary."""
import asyncio
import copy
import sqlite3
from pathlib import Path

import pytest
from model_collaboration_engine import Engine
from test_planning import planning_server  # noqa: F401


def test_feedback_persists_and_rejects_conflicts(planning_server):
    config, submission, _ = planning_server

    async def run():
        async with await Engine.open(config) as engine:
            result = await engine.run(submission)
            metrics = await engine.metrics()
            assert metrics["quality"] == []
            assert sum(c["attempts"] for c in metrics["calls"]) == 2
            assert sum(c["known_cost"] for c in metrics["calls"]) == result["settled_cost"]
            feedback = {
                "feedback_id": "acceptance-1", "task_id": result["task_id"],
                "artifact_id": result["artifact"]["artifact_id"],
                "kind": "business_acceptance", "evaluator_version": "1",
                "accepted": False, "reason": "Host review rejected the answer",
            }
            await asyncio.gather(engine.record_feedback(feedback), engine.record_feedback(feedback))
            metrics = await engine.metrics()
            assert metrics["revision"] == 1
            assert metrics["quality"][0]["samples"] == 1
            assert metrics["quality"][0]["accepted"] == 0
            for patch in [
                {"accepted": True}, {"feedback_id": "duplicate"},
                {"feedback_id": "wrong-version", "evaluator_version": "2"},
                {"feedback_id": "wrong-task", "task_id": "other"},
                {"feedback_id": "no-critic", "kind": "critic_correctness"},
            ]:
                with pytest.raises(RuntimeError, match="feedback"):
                    await engine.record_feedback(feedback | patch)
            records = await engine.evaluations(result["task_id"])
            assert records[0]["artifact"] == result["artifact"]
            assert records[0]["deterministic"]["status"] == "pass"
            assert records[0]["critic"] is None
            next_task = copy.deepcopy(submission)
            next_task["task_id"] += "-next"
            await engine.run(next_task)
        with pytest.raises(RuntimeError, match="closing or closed"):
            await engine.metrics()
        async with await Engine.open(config) as reopened:
            assert await reopened.evaluations(result["task_id"]) == records
            assert (await reopened.metrics())["revision"] == 1

    asyncio.run(run())


def test_old_database_is_rejected_without_reset(planning_server):
    config, _, mode = planning_server
    path = Path(config["database_path"])
    with sqlite3.connect(path) as db:
        db.executescript(
            "CREATE TABLE evidence(value TEXT); INSERT INTO evidence VALUES('keep');"
            "PRAGMA application_id=1296254257; PRAGMA user_version=1;"
        )
    before = path.read_bytes()

    async def run():
        with pytest.raises(RuntimeError, match="backup_and_rebuild"):
            await Engine.open(config)
        assert path.read_bytes() == before
        fresh = config | {"database_path": str(path.with_name("fresh.db"))}
        async with await Engine.open(fresh) as engine:
            assert (await engine.metrics())["tasks"]["total"] == 0
        assert path.read_bytes() == before

    asyncio.run(run())
    assert mode["requests"] == []
