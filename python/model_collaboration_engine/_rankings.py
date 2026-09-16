"""Bounded, offline import of explicitly mapped external ranking evidence."""
import csv
import io
import json
import math
from os import PathLike
from pathlib import Path
from typing import Any, TypedDict

_MAX_BYTES = 1024 * 1024
_MAX_ROWS = 100_000
_METADATA = {"schema_version", "version", "source", "source_version", "category",
             "published_at_ms", "expires_at_ms", "population", "weight"}
_MAPPING = {"source_model", "source_model_version", "model_id", "model_version"}
_TASK_TYPES = {"general", "code_generation", "code_review", "information_extraction",
               "reasoning", "writing", "tool_execution"}


class ImportReport(TypedDict):
    source_rows: int
    mapped_rows: int
    imported_entries: int
    unmapped: list[dict[str, str]]
    unranked: list[dict[str, str]]


class RankingImport(TypedDict):
    rankings: dict[str, Any]
    report: ImportReport


def load_rankings(path: str | PathLike[str], manifest_path: str | PathLike[str]) -> RankingImport:
    """Import local JSON/CSV and an explicit JSON mapping manifest, without network I/O.

    Each input is limited to 1 MiB and 100,000 rows/entries. Return canonical
    ``rankings`` for ``routing.rankings`` and a ``report`` listing unmapped and
    unranked source identities. Missing/null/blank ranks produce no entries.
    Raises ValueError for malformed data and OSError for file access failures.
    Local candidate existence is checked later by the native parse_config loader.
    This synchronous helper should run outside an asynchronous task's hot path.
    """
    spec = _manifest(_json(_read(Path(manifest_path))))
    rows = _rows(Path(path), spec["population"])
    entries = []
    mapped = set()
    local_keys = set()
    for mapping in spec["mappings"]:
        identity = (mapping["source_model"], mapping["source_model_version"])
        if identity not in rows:
            raise ValueError("mapping has no matching source model/version")
        mapped.add(identity)
        for role in spec["roles"]:
            key = (mapping["model_id"], mapping["model_version"], spec["task_type"], role)
            if key in local_keys:
                raise ValueError("duplicate local model/version/task_type/role mapping")
            local_keys.add(key)
            if rows[identity] is not None:
                entries.append({**mapping, "task_type": spec["task_type"],
                                "role": role, "rank": rows[identity]})
            if len(local_keys) > _MAX_ROWS:
                raise ValueError("too many ranking entries")
    return {
        "rankings": {**{key: spec[key] for key in sorted(_METADATA)}, "entries": entries},
        "report": {
            "source_rows": len(rows), "mapped_rows": len(mapped),
            "imported_entries": len(entries),
            "unmapped": [_identity(key) for key in rows if key not in mapped],
            "unranked": [_identity(key) for key, rank in rows.items() if rank is None],
        },
    }


def _identity(key: tuple[str, str]) -> dict[str, str]:
    return {"model": key[0], "version": key[1]}


def _read(path: Path) -> str:
    with path.open("rb") as handle:
        data = handle.read(_MAX_BYTES + 1)
    if len(data) > _MAX_BYTES:
        raise ValueError("ranking input exceeds 1 MiB")
    try:
        return data.decode("utf-8-sig")
    except UnicodeDecodeError as error:
        raise ValueError("ranking input must be UTF-8") from error


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON field")
        result[key] = value
    return result


def _json(text: str) -> Any:
    def reject_constant(value: str) -> None:
        raise ValueError("JSON numbers must be finite")
    try:
        return json.loads(text, object_pairs_hook=_unique_object, parse_constant=reject_constant)
    except (json.JSONDecodeError, RecursionError) as error:
        raise ValueError("invalid ranking JSON") from error


def _fields(value: Any, required: set[str], optional: set[str] | None = None) -> None:
    if not isinstance(value, dict) or not required <= value.keys():
        raise ValueError("missing required ranking fields")
    if value.keys() - required - (optional or set()):
        raise ValueError("unknown ranking fields")


def _text(value: Any, limit: int = 256) -> None:
    if not isinstance(value, str) or not value.strip():
        raise ValueError("ranking identities must be nonempty strings")
    try:
        size = len(value.encode("utf-8"))
    except UnicodeEncodeError as error:
        raise ValueError("ranking identities must be valid UTF-8") from error
    if size > limit:
        raise ValueError("ranking identity exceeds byte limit")


def _integer(value: Any, minimum: int, maximum: int) -> None:
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError("ranking integer is outside its permitted range")


def _manifest(spec: Any) -> dict[str, Any]:
    _fields(spec, _METADATA | {"task_type", "roles", "mappings"})
    _integer(spec["schema_version"], 1, 1)
    for key in ("version", "source_version", "category"):
        _text(spec[key])
    _text(spec["source"], 2048)
    _integer(spec["population"], 1, 1_000_000)
    for key in ("published_at_ms", "expires_at_ms"):
        _integer(spec[key], 0, 2**63 - 1)
    if spec["expires_at_ms"] <= spec["published_at_ms"]:
        raise ValueError("ranking expiry must follow publication")
    weight = spec["weight"]
    if type(weight) not in (int, float) or not 0 <= weight <= 1 or not math.isfinite(weight):
        raise ValueError("ranking weight must be finite and in 0..1")
    _text(spec["task_type"])
    if spec["task_type"] not in _TASK_TYPES:
        raise ValueError("unknown ranking task_type")
    roles = spec["roles"]
    if not isinstance(roles, list) or not roles or len(roles) > 3:
        raise ValueError("ranking roles must be a nonempty list")
    for role in roles:
        _text(role)
        if role not in {"invoke", "generator", "critic"}:
            raise ValueError("unknown ranking role")
    if len(set(roles)) != len(roles):
        raise ValueError("duplicate ranking role")
    mappings = spec["mappings"]
    if not isinstance(mappings, list) or len(mappings) > _MAX_ROWS:
        raise ValueError("invalid ranking mappings list")
    for mapping in mappings:
        _fields(mapping, _MAPPING)
        for value in mapping.values():
            _text(value)
    return spec


def _rows(path: Path, population: int) -> dict[tuple[str, str], int | None]:
    text = _read(path)
    is_csv = path.suffix.lower() == ".csv"
    if is_csv:
        try:
            reader = csv.reader(io.StringIO(text, newline=""), strict=True)
            if next(reader, None) != ["model", "version", "rank"]:
                raise ValueError("CSV header must be model,version,rank")
            data = []
            for row in reader:
                if len(row) != 3:
                    raise ValueError("CSV rows must contain exactly three columns")
                data.append(dict(zip(("model", "version", "rank"), row, strict=True)))
                if len(data) > _MAX_ROWS:
                    raise ValueError("too many source rows")
        except csv.Error as error:
            raise ValueError("invalid ranking CSV") from error
    elif path.suffix.lower() == ".json":
        data = _json(text)
    else:
        raise ValueError("ranking source must have .json or .csv extension")
    if not isinstance(data, list) or len(data) > min(_MAX_ROWS, population):
        raise ValueError("ranking source must be an array of at most 100,000 rows")
    rows = {}
    for row in data:
        _fields(row, {"model", "version"}, {"rank"})
        _text(row["model"])
        _text(row["version"])
        identity = (row["model"], row["version"])
        if identity in rows:
            raise ValueError("duplicate source model/version")
        rank = row.get("rank")
        if rank is None or (isinstance(rank, str) and not rank.strip()):
            rank = None
        else:
            if is_csv:
                if not rank.isascii() or not rank.isdecimal():
                    raise ValueError("CSV rank must be a positive integer or blank")
                rank = int(rank)
            _integer(rank, 1, population)
        rows[identity] = rank
    return rows
