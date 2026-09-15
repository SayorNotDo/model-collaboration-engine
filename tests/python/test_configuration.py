"""Layered service configuration through the native loader and real local HTTP fixtures."""
import asyncio
import copy
import json
from pathlib import Path

import pytest
from test_planning import planning_server as planning_server

from model_collaboration_engine import Engine, load_config, parse_config

ROOT = Path(__file__).resolve().parents[2]


def document():
    old = json.loads((ROOT / "examples/config.json").read_text(encoding="utf-8"))
    model = old["models"][0]
    connection = {"base_url", "api_key_env", "provider", "region", "local"}
    return {
        "schema_version": 1, "storage": {"database_path": "engine.db"},
        "providers": [{"id": "local", "kind": "direct", "base_url": model["base_url"],
                       "auth": {"type": "bearer", "env": model["api_key_env"]},
                       "endpoints": ["chat_completions", "responses"],
                       "region": "local", "local": True,
                       "models": [{k: v for k, v in model.items() if k not in connection}]}],
        "routing": {"weights": old["weights"]},
        "runtime": {k: v for k, v in old.items() if k not in ("models", "weights", "database_path")},
    }


def test_file_paths_and_immutable_detached_configuration(tmp_path):
    doc = document()
    doc["storage"]["database_path"] = "new.db"
    path = tmp_path / "engine.json"
    path.write_text(json.dumps(doc), encoding="utf-8")
    loaded = load_config(path)
    assert Path(loaded.database_path) == tmp_path / "new.db"
    assert loaded.model_ids == ["local"]
    with pytest.raises(AttributeError):
        loaded.database_path = "other.db"
    ids = loaded.model_ids
    ids.append("injected")
    assert loaded.model_ids == ["local"]
    parsed = parse_config(doc, base_dir=tmp_path)
    doc["providers"][0]["models"][0]["id"] = "changed"
    assert parsed.model_ids == ["local"]
    assert not (tmp_path / "new.db").exists()


def test_parse_error_reports_field_without_value(tmp_path):
    doc = document()
    doc["providers"][0]["auth"]["type"] = "secret-never-echo"
    with pytest.raises(RuntimeError) as failure:
        parse_config(doc, base_dir=tmp_path)
    data = json.loads(str(failure.value))
    assert data["kind"] == "configuration"
    assert data["details"]["field"] == "providers[0].auth.type"
    assert "secret-never-echo" not in str(failure.value)


def test_gateway_alias_routes_through_resolved_connection(planning_server, tmp_path):
    effective, task, server = planning_server
    doc = document()
    doc["storage"]["database_path"] = str(tmp_path / "gateway.db")
    provider = doc["providers"][0]
    provider.update(id="relay", kind="relay", local=False,
                    base_url=effective["models"][0]["base_url"])
    model = doc["providers"][0]["models"][0]
    model.update(model="upstream/alias",
                 endpoint=effective["models"][0]["endpoint"])
    second = copy.deepcopy(model)
    second.update(id="other", model="another-model")
    doc["providers"][0]["models"].append(second)
    task.pop("planning", None)
    task["strategy"] = "single"
    task["constraints"]["local_only"] = False
    task["constraints"]["allowed_providers"] = ["relay"]
    task["constraints"]["allowed_models"] = ["local"]
    config = parse_config(doc, base_dir=tmp_path)

    async def run():
        async with await Engine.open(config) as engine:
            result = await engine.run(task)
            assert result["status"] == "completed"
            assert result["settled_cost"] == 15
            assert len(server["requests"]) == 1
            assert server["requests"][0][1]["model"] == "upstream/alias"
    asyncio.run(run())
