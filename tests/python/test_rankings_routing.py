"""External preference reaches native routing without changing business quality."""
import asyncio
import copy
import json
import sqlite3
import time
from contextlib import closing

import pytest
from test_configuration import document
from test_planning import planning_server as planning_server

from model_collaboration_engine import Engine, parse_config


def rankings():
    now = int(time.time() * 1000)
    return {
        "schema_version": 1, "version": "synthetic-v1", "source": "synthetic fixture",
        "source_version": "benchmark-v1", "category": "synthetic reasoning",
        "published_at_ms": now - 1000, "expires_at_ms": now + 60000,
        "population": 2, "weight": 0.5,
        "entries": [{"model_id": name, "model_version": "1", "source_model": name,
                     "source_model_version": "external-v1", "task_type": "reasoning",
                     "role": "invoke", "rank": rank}
                    for name, rank in (("local", 2), ("other", 1))],
    }


def test_rank_preference_respects_constraints_and_task_type(planning_server):
    config, submission, mode = planning_server
    other = copy.deepcopy(config["models"][0])
    other.update(id="other", model="ranked-model")
    config["models"].append(other)
    config["rankings"] = rankings()

    async def run():
        async with await Engine.open(config) as engine:
            for identity in ("ranked", "constrained", "different-type"):
                task = copy.deepcopy(submission)
                task.update(task_id=identity, task_type="reasoning", strategy="single")
                if identity == "constrained":
                    task["constraints"]["allowed_models"] = ["local"]
                elif identity == "different-type":
                    task["task_type"] = "writing"
                result = await engine.run(task)
                assert result["status"] == "completed"
                assert result["settled_cost"] == 15
                assert result["reserved_cost"] == 0

    asyncio.run(run())
    assert [request["model"] for _, request in mode["requests"]] == [
        "ranked-model", config["models"][0]["model"], config["models"][0]["model"],
    ]
    with closing(sqlite3.connect(config["database_path"])) as db:
        routes = {task: json.loads(metadata)["route"] for task, metadata in db.execute(
            "SELECT task,metadata FROM attempts")}
        snapshot = json.loads(db.execute(
            "SELECT plan FROM tasks WHERE id='ranked'").fetchone()[0])["routing_snapshot"]
    assert routes["ranked"]["breakdown"]["ranking"] == 0.5
    assert routes["ranked"]["ranking"]["status"] == "applied"
    assert routes["ranked"]["quality"]["quality"] == config["models"][1]["acceptance"]
    assert routes["different-type"]["breakdown"].get("ranking", 0) == 0
    assert snapshot["ranking_config"]["source"] == "synthetic fixture"
    assert len(snapshot["ranking_config_hash"]) == 64


def test_nested_configuration_validates_local_identity(tmp_path):
    config = document()
    block = rankings()
    block["entries"] = block["entries"][:1]
    config["routing"]["rankings"] = block
    assert parse_config(config, base_dir=tmp_path).model_ids == ["local"]
    block["entries"][0]["model_id"] = "missing"
    with pytest.raises(RuntimeError, match="configuration"):
        parse_config(config, base_dir=tmp_path)
    assert not (tmp_path / "engine.db").exists()
