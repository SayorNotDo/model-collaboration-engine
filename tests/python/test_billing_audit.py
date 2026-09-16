"""Offline billing reports distinguish estimates from supplier debits."""
import importlib.util
import json
from pathlib import Path

import pytest

path = Path(__file__).resolve().parents[2] / "examples" / "billing_audit.py"
spec = importlib.util.spec_from_file_location("billing_audit", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def evidence():
    return {
        "request_id": "synthetic-request", "model_version": "synthetic-v1",
        "usage": {"prompt_tokens": 1000, "completion_tokens": 100,
                  "prompt_cache_hit_tokens": 800, "prompt_cache_miss_tokens": 200},
        "pricing": {"currency": "CNY", "version": "synthetic-rate-v1",
                    "source": "synthetic test fixture", "band": "off_peak",
                    "band_evidence": "synthetic supplier classification",
                    "cache_hit_per_million": "0.1", "cache_miss_per_million": "1",
                    "output_per_million": "2"},
    }


def test_cache_split_reprices_without_claiming_invoice_verification():
    report = module.reprice_call(evidence())
    assert report["calculated_amount"] == "0.00048"
    assert report["supplier_bill_verified"] is False


@pytest.mark.parametrize("field", ["prompt_cache_hit_tokens", "prompt_cache_miss_tokens"])
def test_missing_cache_evidence_is_not_zero(field):
    value = evidence()
    del value["usage"][field]
    with pytest.raises(ValueError, match=field):
        module.reprice_call(value)


@pytest.mark.parametrize("value", [-1, True, 0.5, "3"])
def test_invalid_usage_is_rejected(value):
    record = evidence()
    record["usage"]["prompt_tokens"] = value
    with pytest.raises(ValueError):
        module.reprice_call(record)


def test_inconsistent_cache_split_is_rejected():
    record = evidence()
    record["usage"]["prompt_cache_hit_tokens"] = 799
    with pytest.raises(ValueError, match="sum"):
        module.reprice_call(record)


@pytest.mark.parametrize("rate", ["NaN", "Infinity", "-1", 0.1])
def test_invalid_or_float_rates_are_rejected(rate):
    record = evidence()
    record["pricing"]["output_per_million"] = rate
    with pytest.raises(ValueError):
        module.reprice_call(record)


def test_time_band_cannot_be_inferred_from_archive_folder():
    record = evidence()
    del record["pricing"]["band_evidence"]
    with pytest.raises(ValueError, match="band_evidence"):
        module.reprice_call(record)


def test_archive_report_preserves_unresolved_cost_and_no_supplier_total(tmp_path):
    (tmp_path / "results").mkdir()
    (tmp_path / "pricing.json").write_text(json.dumps({"basis": "upper estimate"}))
    (tmp_path / "results" / "single.metrics.json").write_text(json.dumps({"calls": [
        {"attempts": 2, "known_cost": 25, "unresolved_reserved": 30,
         "unknown_cost_attempts": 1},
    ]}))
    report = module.audit_archive(tmp_path)
    assert report["ledger_estimate_microcredits"] == 25
    assert report["unresolved_reserved_microcredits"] == 30
    assert report["unknown_cost_attempts"] == 1
    assert report["supplier_amount"] is None
    assert report["supplier_bill_verified"] is False


@pytest.mark.parametrize("value", [None, [], "invalid", 1])
def test_non_object_call_evidence_is_rejected(value):
    with pytest.raises(ValueError, match="object"):
        module.reprice_call(value)


@pytest.mark.parametrize("field", ["usage", "pricing"])
@pytest.mark.parametrize("value", [None, [], "invalid"])
def test_non_object_nested_evidence_is_rejected(field, value):
    record = evidence()
    record[field] = value
    with pytest.raises(ValueError, match=field):
        module.reprice_call(record)


@pytest.mark.parametrize("payload", [b"{", b"\xff", b"[]", b"null"])
def test_invalid_evidence_file_is_rejected(tmp_path, payload):
    path = tmp_path / "evidence.json"
    path.write_bytes(payload)
    with pytest.raises(ValueError):
        module.load_evidence(path)


def test_file_size_is_bounded(tmp_path):
    path = tmp_path / "evidence.json"
    path.write_bytes(b" " * (1024 * 1024 + 1))
    with pytest.raises(ValueError, match="1 MiB"):
        module.load_evidence(path)


@pytest.mark.parametrize("snapshot", [{}, {"calls": None}, {"calls": [None]}])
def test_malformed_archive_metrics_are_rejected(tmp_path, snapshot):
    (tmp_path / "results").mkdir()
    (tmp_path / "results" / "single.metrics.json").write_text(json.dumps(snapshot))
    with pytest.raises(ValueError, match="calls"):
        module.audit_archive(tmp_path)
