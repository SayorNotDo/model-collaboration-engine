"""Validate an evaluation before allocating output or dispatching paid work."""
import copy
import json
import os
import time
from pathlib import Path
from typing import Any

from model_collaboration_engine import parse_config


def read_document(path: Path) -> dict[str, Any]:
    if path.stat().st_size > 1_048_576:
        raise ValueError("input exceeds 1 MiB")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("input must be an object")
    return value


def preflight(config_path: Path, suite_path: Path, total_budget: int) -> tuple[dict, dict]:
    config, suite = read_document(config_path), read_document(suite_path)
    parse_config(config, base_dir=config_path.parent)
    models = [model for provider in config["providers"] for model in provider["models"]]
    for model in models:
        for field in ("id", "model", "version", "endpoint", "price_version"):
            value = model.get(field)
            if (not isinstance(value, str) or not value.strip()
                    or any(marker in value.lower() for marker in ("replace", "placeholder", "example"))):
                raise ValueError(f"model requires an explicit non-placeholder {field}")
    if len(models) < 2:
        raise ValueError("strategy comparison requires at least two model candidates")
    for provider in config["providers"]:
        name = provider["auth"]["env"]
        if not os.environ.get(name, "").strip():
            raise ValueError(f"missing credential environment variable: {name}")
    validate_suite(suite)
    required = (2 * len(suite["cases"]) + 1) * suite["budget"]
    if type(total_budget) is not int or not 0 < total_budget <= 2**63 - 1:
        raise ValueError("total budget must be positive integer microcredits")
    if required > total_budget:
        raise ValueError(f"planned admission requires {required} microcredits")
    return config, suite


def validate_suite(suite: dict) -> None:
    if not isinstance(suite.get("version"), str) or not 1 <= len(suite["version"]) <= 128:
        raise ValueError("suite requires a bounded evaluator version")
    limits = {"budget": 2**63-1, "max_call_cost": 2**63-1, "output_tokens": 1_000_000,
              "timeout_ms": 86_400_000, "max_calls": 1000, "max_rounds": 100,
              "max_attempts": 100}
    for key, maximum in limits.items():
        value = suite.get(key)
        if type(value) is not int or not 1 <= value <= maximum:
            raise ValueError(f"invalid suite {key}")
    if suite["timeout_ms"] <= 1000 or suite["max_call_cost"] > suite["budget"]:
        raise ValueError("timeout must exceed finalization; call cost must fit task budget")
    cases = suite.get("cases")
    if not isinstance(cases, list) or not 1 <= len(cases) <= 100:
        raise ValueError("suite must contain 1 to 100 cases")
    ids = set()
    for case in cases:
        if not isinstance(case, dict):
            raise ValueError("case must be an object")
        identity = case.get("id")
        if not isinstance(identity, str) or not 1 <= len(identity) <= 64 or identity in ids:
            raise ValueError("case IDs must be unique bounded strings")
        ids.add(identity)
        if not isinstance(case.get("input"), str) or not case["input"].strip():
            raise ValueError("case input must be nonempty text")
        expected = case.get("expected")
        if not isinstance(expected, dict) or not expected:
            raise ValueError("expected must be a nonempty JSON object")
        if any(type(v) not in (str, int, bool, type(None)) for v in expected.values()):
            raise ValueError("reference values must be strings, integers, booleans or null")


def group_config(config: dict, output: Path, group: str) -> dict:
    document = copy.deepcopy(config)
    document["storage"]["database_path"] = str(output / f"{group}.db")
    return document


def submission(suite: dict, case: dict, group: str, ordinal: int) -> dict:
    fields = {key: {str: "string", int: "integer", bool: "boolean", type(None): "null"}[type(v)]
              for key, v in case["expected"].items()}
    return {
        "task_id": f"{group}-{ordinal}", "task_type": "information_extraction",
        "strategy": "single" if group == "smoke" else group,
        "planning": {"mode": "disabled", "max_cost": 0, "timeout_ms": 5000},
        "goal": "Extract the fields from the source. Return only a JSON object with exactly "
                f"these fields and types: {json.dumps(fields, ensure_ascii=False)}.\n"
                f"Source:\n{case['input']}",
        "evidence": [], "tools": [],
        "acceptance": {"version": suite["version"], "nonempty": True,
                       "json_object": True, "required_substrings": []},
        "selection": {"version": "evaluation-selection-v1", "min_quality": 0.0,
                      "target_quality": 0.9, "above_target_factor": 0.1,
                      "cost_reference": suite["max_call_cost"],
                      "latency_reference_ms": 5000, "min_upgrade_gain": 0.05},
        "constraints": {"allowed_models": [], "allowed_providers": [], "allowed_regions": [],
                        "local_only": False, "required_capabilities": ["text", "json"],
                        "preferred_capabilities": [], "different_critic": False},
        "deadline_ms": int(time.time() * 1000) + suite["timeout_ms"], "finalization_ms": 1000,
        **{key: suite[key] for key in ("budget", "max_call_cost", "output_tokens", "max_calls",
                                      "max_rounds", "max_attempts")},
    }
