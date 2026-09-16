import json
import subprocess
import sys
from pathlib import Path

import pytest

from model_collaboration_engine import load_rankings

ROOT = Path(__file__).resolve().parents[2]


def manifest():
    return {
        "schema_version": 1, "version": "synthetic-v1", "source": "synthetic fixture",
        "source_version": "v1", "category": "synthetic reasoning", "published_at_ms": 1,
        "expires_at_ms": 1000, "population": 10, "weight": 0.2,
        "task_type": "reasoning", "roles": ["invoke", "generator"],
        "mappings": [{"source_model": "external", "source_model_version": "v1",
                      "model_id": "local", "model_version": "v2"}],
    }


def load(tmp_path, rows=None, spec=None, csv=None):
    source = tmp_path / ("source.csv" if csv is not None else "source.json")
    source.write_text(csv if csv is not None else json.dumps(rows), encoding="utf-8-sig")
    path = tmp_path / "manifest.json"
    path.write_text(json.dumps(spec or manifest()), encoding="utf-8")
    return load_rankings(source, path)


def test_json_csv_equivalent_explicit_versions_and_roles(tmp_path):
    rows = [{"model": "external", "version": "v1", "rank": 2},
            {"model": "other", "version": "v1", "rank": None}]
    result = load(tmp_path, rows)
    assert result == load(tmp_path, csv="model,version,rank\nexternal,v1,2\nother,v1,\n")
    assert [entry["role"] for entry in result["rankings"]["entries"]] == ["invoke", "generator"]
    assert result["rankings"]["entries"][0]["model_version"] == "v2"
    assert result["report"]["source_rows"] == 2
    assert result["report"]["unmapped"] == [{"model": "other", "version": "v1"}]
    assert result["report"]["unranked"] == [{"model": "other", "version": "v1"}]


def test_missing_rank_is_unknown_not_zero(tmp_path):
    assert load(tmp_path, [{"model": "external", "version": "v1"}])["rankings"]["entries"] == []


@pytest.mark.parametrize("rank", [0, -1, 11, True, 1.5, "2", float("nan")])
def test_invalid_rank_even_when_unmapped(tmp_path, rank):
    with pytest.raises(ValueError):
        load(tmp_path, [{"model": "other", "version": "v1", "rank": rank}])


@pytest.mark.parametrize("rows", [
    [{"model": "other", "version": "v1", "rank": 1}] * 2,
    [{"model": "external", "version": "v1", "rank": 1, "extra": 2}],
    [{"model": "external", "rank": 1}],
    {},
])
def test_malformed_rows(tmp_path, rows):
    with pytest.raises(ValueError):
        load(tmp_path, rows)


@pytest.mark.parametrize("csv", ["model,version,rank,rank\na,v,1,1\n",
                                 "model,version,rank\na,v,1,extra\n",
                                 "model,version,rank\na,v,1.0\n"])
def test_malformed_csv(tmp_path, csv):
    with pytest.raises(ValueError):
        load(tmp_path, csv=csv)


@pytest.mark.parametrize(("key", "value"), [
    ("weight", float("inf")), ("weight", True), ("weight", -0.1),
    ("population", 0), ("published_at_ms", True), ("expires_at_ms", 1),
    ("expires_at_ms", 2**63), ("category", ""), ("extra", 1),
    ("roles", ["invoke", "invoke"]), ("roles", ["unknown"]), ("task_type", "unknown"),
])
def test_invalid_manifest(tmp_path, key, value):
    spec = manifest()
    spec[key] = value
    with pytest.raises(ValueError):
        load(tmp_path, [], spec)


def test_unresolved_and_duplicate_mapping_rejected(tmp_path):
    with pytest.raises(ValueError):
        load(tmp_path, [])
    spec = manifest()
    spec["mappings"] *= 2
    with pytest.raises(ValueError):
        load(tmp_path, [{"model": "external", "version": "v1", "rank": 1}], spec)


def test_cli_creates_new_output_and_refuses_overwrite(tmp_path):
    output = tmp_path / "rankings.json"
    report = tmp_path / "report.json"
    command = [sys.executable, str(ROOT / "examples/import_rankings.py"),
               "--input", str(ROOT / "examples/rankings/models.csv"),
               "--manifest", str(ROOT / "examples/rankings/manifest.json"),
               "--output", str(output), "--report", str(report)]
    subprocess.run(command, check=True, capture_output=True)
    original = output.read_bytes()
    assert json.loads(original)["source"] == "synthetic fixture"
    assert json.loads(report.read_text())["source_rows"] == 3
    assert subprocess.run(command, capture_output=True).returncode != 0
    assert output.read_bytes() == original


def test_population_counts_unknown_rows_and_allows_ties_and_provider_mappings(tmp_path):
    rows = [{"model": "external", "version": "v1", "rank": 1},
            {"model": "other", "version": "v1", "rank": None}]
    spec = manifest()
    spec["population"] = 1
    with pytest.raises(ValueError):
        load(tmp_path, rows, spec)
    spec["population"] = 2
    rows[1]["rank"] = 1
    spec["mappings"].append({**spec["mappings"][0], "model_id": "second-provider"})
    result = load(tmp_path, rows, spec)
    assert len(result["rankings"]["entries"]) == 4
    assert result["report"]["mapped_rows"] == 1


def test_input_size_limit(tmp_path):
    source = tmp_path / "source.json"
    source.write_bytes(b" " * (1024 * 1024 + 1))
    spec = tmp_path / "manifest.json"
    spec.write_text(json.dumps(manifest()))
    with pytest.raises(ValueError, match="1 MiB"):
        load_rankings(source, spec)


def test_duplicate_json_fields_rejected(tmp_path):
    source = tmp_path / "source.json"
    source.write_text('[{"model":"external","version":"v1","rank":1,"rank":2}]')
    spec = tmp_path / "manifest.json"
    spec.write_text(json.dumps(manifest()))
    with pytest.raises(ValueError, match="duplicate JSON"):
        load_rankings(source, spec)
