"""Task-fit selection reaches the installed native router through both protocols."""
import asyncio
import copy
import json
import sqlite3

from test_planning import planning_server as planning_server

from model_collaboration_engine import Engine


def configure_candidates(config, economic_quality, strong_quality):
    economic = config["models"][0]
    economic.update(
        id="economic",
        model="economic-model",
        acceptance=economic_quality,
        input_price=1,
        output_price=1,
    )
    strong = copy.deepcopy(economic)
    strong.update(
        id="strong",
        model="strong-model",
        acceptance=strong_quality,
        input_price=4,
        output_price=4,
    )
    config["models"] = [economic, strong]
    config["planner_models"] = []
    config["weights"] = {
        "quality": 1.0,
        "capability": 0.0,
        "reliability": 0.0,
        "cost": 0.1,
        "latency": 0.0,
        "uncertainty": 0.0,
        "version": "task-fit-native-v1",
    }


def explicit_task(submission, identity, budget):
    task = copy.deepcopy(submission)
    task.update(
        task_id=identity,
        task_type="reasoning",
        strategy="single",
        budget=budget,
        max_call_cost=budget,
    )
    task["planning"] = {"mode": "disabled", "max_cost": 0, "timeout_ms": 5000}
    task["selection"] = {
        "version": "host-task-fit-v1",
        "min_quality": 0.0,
        "target_quality": 0.9,
        "above_target_factor": 0.1,
        "cost_reference": 10_000,
        "latency_reference_ms": 5_000,
        "min_upgrade_gain": 0.05,
    }
    return task


def test_simple_task_prefers_economic_model_independent_of_budget(planning_server):
    config, submission, mode = planning_server
    configure_candidates(config, 0.90, 0.95)

    async def run():
        async with await Engine.open(config) as engine:
            for identity, budget in (("normal-budget", 10_000), ("wide-budget", 100_000)):
                result = await engine.run(explicit_task(submission, identity, budget))
                assert result["status"] == "completed"

    asyncio.run(run())
    assert [payload["model"] for _, payload in mode["requests"]] == [
        "economic-model",
        "economic-model",
    ]
    with sqlite3.connect(config["database_path"]) as database:
        plans = [json.loads(row[0]) for row in database.execute("SELECT plan FROM tasks")]
    assert all(plan["routing_snapshot"]["schema_version"] == 4 for plan in plans)
    assert all(
        plan["routing_snapshot"]["algorithm_version"] == "task-fit-v1" for plan in plans
    )


def test_difficult_task_can_select_strong_model_on_first_call(planning_server):
    config, submission, mode = planning_server
    configure_candidates(config, 0.76, 0.95)

    async def run():
        async with await Engine.open(config) as engine:
            result = await engine.run(explicit_task(submission, "difficult", 10_000))
            assert result["status"] == "completed"

    asyncio.run(run())
    assert [payload["model"] for _, payload in mode["requests"]] == ["strong-model"]
