"""Offline archive audit or explicit cache/time-band repricing; never changes the ledger.

python examples/billing_audit.py artifacts/live-20260916-020845
python examples/billing_audit.py --call-evidence call-evidence.json
"""
import argparse
import json
from decimal import Decimal, InvalidOperation, localcontext
from pathlib import Path
from typing import Any


def _object(value: Any, field: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{field} must be a JSON object")
    return value


def load_evidence(path: Path) -> dict[str, Any]:
    """Read a JSON object up to 1 MiB; malformed evidence raises ValueError."""
    with path.open("rb") as source:
        raw = source.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise ValueError("evidence file exceeds 1 MiB")
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as error:
        raise ValueError("evidence file must contain valid UTF-8 JSON") from error
    return _object(value, "evidence")


def _count(value: Any, field: str) -> int:
    if type(value) is not int or value < 0:
        raise ValueError(f"{field} must be a nonnegative integer")
    return value


def _text(record: dict[str, Any], field: str) -> str:
    value = record.get(field)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{field} requires explicit evidence")
    return value


def _rate(record: dict[str, Any], field: str) -> Decimal:
    value = _text(record, field)
    try:
        rate = Decimal(value)
    except InvalidOperation as error:
        raise ValueError(f"{field} must be a decimal string") from error
    if not rate.is_finite() or rate < 0 or rate.adjusted() > 12 or rate.as_tuple().exponent < -12:
        raise ValueError(f"{field} must be finite, nonnegative, and within supported precision")
    return rate


def reprice_call(record: dict[str, Any]) -> dict[str, Any]:
    """Calculate a supplied tariff, without authenticating evidence or invoice settlement.

    All three token categories and the applicable price band are mandatory. Rates
    are currency units per million tokens, represented as decimal strings. No
    supplier rounding rule, currency conversion or ledger adjustment is assumed.
    """
    record = _object(record, "record")
    identity = {key: _text(record, key) for key in ("request_id", "model_version")}
    usage = _object(record.get("usage"), "usage")
    counts = {key: _count(usage.get(key), key) for key in (
        "prompt_tokens", "completion_tokens", "prompt_cache_hit_tokens", "prompt_cache_miss_tokens",
    )}
    if counts["prompt_cache_hit_tokens"] + counts["prompt_cache_miss_tokens"] != counts["prompt_tokens"]:
        raise ValueError("cache hit and miss counts must sum to prompt_tokens")
    pricing = _object(record.get("pricing"), "pricing")
    metadata = {key: _text(pricing, key) for key in (
        "currency", "version", "source", "band", "band_evidence",
    )}
    if metadata["band"] not in ("peak", "off_peak", "flat"):
        raise ValueError("band must be peak, off_peak or flat")
    categories = (
        ("prompt_cache_hit_tokens", "cache_hit_per_million"),
        ("prompt_cache_miss_tokens", "cache_miss_per_million"),
        ("completion_tokens", "output_per_million"),
    )
    rates = {field: _rate(pricing, field) for _, field in categories}
    with localcontext() as context:
        context.prec = max(50, len(str(max(counts.values()))) + 40)
        amount = sum(Decimal(counts[token]) * rates[rate] for token, rate in categories)
        amount /= Decimal(1_000_000)
    return identity | {
        "pricing": metadata | {key: str(value) for key, value in rates.items()},
        "usage": counts,
        "calculated_amount": format(amount, "f"),
        "supplier_bill_verified": False,
        "status": "tariff_calculation_only",
        "missing_evidence": ["matched supplier invoice/debit and supplier rounding rules"],
    }


def audit_archive(directory: Path) -> dict[str, Any]:
    """Summarize disjoint cohort metrics; missing supplier evidence remains unknown."""
    paths = sorted((directory / "results").glob("*.metrics.json"))
    if not paths:
        raise ValueError("archive has no results/*.metrics.json")
    totals = {key: 0 for key in (
        "attempts", "known_cost", "unresolved_reserved", "unknown_cost_attempts",
    )}
    for path in paths:
        snapshot = load_evidence(path)
        calls = snapshot.get("calls")
        if not isinstance(calls, list):
            raise ValueError("calls must be a JSON array")
        for call in calls:
            call = _object(call, "calls entry")
            for key in totals:
                totals[key] += _count(call.get(key), key)
    pricing = load_evidence(directory / "pricing.json")
    return {
        "archive": str(directory), "metric_files": [path.name for path in paths],
        "cohort_assumption": "each metrics file covers disjoint calls",
        "pricing_evidence": pricing,
        "attempts": totals["attempts"],
        "ledger_estimate_microcredits": totals["known_cost"],
        "unresolved_reserved_microcredits": totals["unresolved_reserved"],
        "unknown_cost_attempts": totals["unknown_cost_attempts"],
        "supplier_amount": None, "supplier_bill_verified": False,
        "status": "insufficient_supplier_evidence",
        "missing_evidence": [
            "per-request cache-hit/cache-miss/output usage",
            "applicable model tariff version and supplier billing time band",
            "matched supplier invoice/debit and supplier rounding rules",
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, nargs="?")
    parser.add_argument("--call-evidence", type=Path)
    args = parser.parse_args()
    if (args.archive is None) == (args.call_evidence is None):
        parser.error("supply exactly one archive directory or --call-evidence JSON")
    if args.call_evidence:
        report = reprice_call(load_evidence(args.call_evidence))
    else:
        report = audit_archive(args.archive)
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
