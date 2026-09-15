"""Static profiles through both native HTTP adapters, without external providers."""
import asyncio
import copy
import json
import sqlite3
from pathlib import Path

import pytest
from test_planning import planning_server as planning_server

from model_collaboration_engine import Engine


def typed_config(config):
    other = copy.deepcopy(config["models"][0])
    other.update(id="other", model="other-served-model")
    config["models"].append(other)
    root = Path(__file__).resolve().parents[2]
    profiles = json.loads((root / "examples/routing-profiles.json").read_text())
    profiles["weights"] = {}
    profiles["profiles"] = [
        {
            "model_id": model, "model_version": "1", "task_type": kind, "role": "invoke",
            "evaluator_version": "1", "prior": quality, "prior_weight": 10,
            "accepted": 0, "samples": 0,
        }
        for model, kind, quality in [
            ("local", "writing", 0.1), ("other", "writing", 0.95),
            ("local", "reasoning", 0.95), ("other", "reasoning", 0.1),
        ]
    ]
    config["routing_profiles"] = profiles


@pytest.mark.parametrize("streaming", [False, True])
def test_one_entry_uses_typed_or_general_snapshot(planning_server, streaming):
    config, submission, mode = planning_server
    typed_config(config)

    async def run():
        async with await Engine.open(config) as engine:
            for kind in ["writing", "reasoning"]:
                task = copy.deepcopy(submission)
                task.update(task_id=kind, task_type=kind, strategy="single")
                if streaming:
                    async with engine.stream(task) as stream:
                        async for _ in stream:
                            pass
                        result = await stream.result()
                else:
                    result = await engine.run(task)
                assert result["status"] == "completed"
            legacy = copy.deepcopy(submission)
            legacy.pop("schema_version")
            legacy.pop("planning")
            legacy.update(task_id="legacy", strategy="single")
            assert (await engine.run(legacy))["status"] == "completed"

    asyncio.run(run())
    assert [p["model"] for _, p in mode["requests"]] == [
        "other-served-model", config["models"][0]["model"], config["models"][0]["model"],
    ]
    assert not any(planner for planner, _ in mode["requests"])
    with sqlite3.connect(config["database_path"]) as db:
        plans = {task_id: json.loads(plan) for task_id, plan in db.execute("SELECT id,plan FROM tasks")}
        assert plans["writing"]["routing_snapshot"]["profile_version"] == "example-static-v1"
        assert plans["legacy"]["routing_snapshot"]["profile_version"] == "example-static-v1"
        assert plans["legacy"]["effective_plan"]["task_type"] == "general"
        rows = db.execute("SELECT task,metadata FROM attempts").fetchall()
        for task_id, metadata in rows:
            route = json.loads(metadata)["route"]
            if task_id == "legacy":
                assert route["quality"]["fallback"] == "global_prior"
                assert route["quality"]["requested"]["task_type"] == "general"
            else:
                assert route["quality"]["fallback"] == "exact"
                assert route["quality"]["requested"]["task_type"] == task_id
                assert len(route["routing_snapshot"]) == 64


@pytest.mark.parametrize("failure", ["cycle", "count", "unknown_field"])
def test_profile_configuration_rejected_before_http(planning_server, failure):
    config, _, mode = planning_server
    typed_config(config)
    profiles = config["routing_profiles"]
    if failure == "cycle":
        profiles["parents"] = {"writing": "reasoning", "reasoning": "writing"}
    elif failure == "count":
        profiles["profiles"][0]["accepted"] = 1
    else:
        profiles["profiles"][0]["untrusted_score"] = 1

    async def run():
        expected = "unknown field `untrusted_score`" if failure == "unknown_field" else "configuration"
        with pytest.raises(Exception, match=expected):
            await Engine.open(config)

    asyncio.run(run())
    assert mode["requests"] == []
