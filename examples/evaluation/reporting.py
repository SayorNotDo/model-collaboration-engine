"""Business acceptance and reports retain denominators and unknown billing evidence."""
import json
from pathlib import Path
from typing import Any

from compare_metrics import summarize


def write_json(path: Path, value: Any) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n",
                         encoding="utf-8")
    temporary.replace(path)


def accepts(text: str, expected: dict) -> bool:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate key")
            result[key] = value
        return result

    try:
        actual = json.loads(text, object_pairs_hook=unique)
    except (ValueError, TypeError):
        return False
    return (isinstance(actual, dict) and actual.keys() == expected.keys()
            and all(type(actual[k]) is type(v) and actual[k] == v for k, v in expected.items()))


def safe_error(error: Exception) -> str:
    # Provider messages can contain arbitrary text. Export only recognized categories.
    try:
        kind = json.loads(str(error)).get("kind")
    except (ValueError, AttributeError):
        kind = None
    allowed = {"budget", "deadline", "cancelled", "routing", "protocol", "transport",
               "configuration", "storage", "closed", "cleanup_timeout"}
    return kind if isinstance(kind, str) and kind in allowed else "execution_error"


def group_report(rows: list[dict], metrics: dict | None) -> dict:
    executed = [r for r in rows if r["status"] != "not_executed"]
    evaluated = [r for r in rows if r.get("accepted") is not None]
    correct = sum(r["accepted"] for r in evaluated)
    return {
        "planned": len(rows), "correct": correct,
        "correct_per_planned": correct / len(rows) if rows else None,
        "evaluated": len(evaluated),
        "artifact_acceptance_rate": correct / len(evaluated) if evaluated else None,
        "no_artifact": sum(not r.get("artifact") for r in executed),
        "not_executed": len(rows) - len(executed),
        "mean_task_elapsed_ms": (sum(r["elapsed_ms"] for r in executed) / len(executed)
                                 if executed else None),
        "metrics": summarize(metrics) if metrics is not None else None,
    }
