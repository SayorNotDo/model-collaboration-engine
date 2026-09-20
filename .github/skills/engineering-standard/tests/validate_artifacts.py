"""Optional artifact validation; requires PyYAML and jsonschema only for development."""
import json
import runpy
import tempfile
from pathlib import Path

import jsonschema
import yaml

BASE = Path(__file__).resolve().parents[1]
scanner = runpy.run_path(str(BASE / "scripts/scan_project.py"))
schema = yaml.safe_load((BASE / "references/project-profile.schema.yaml").read_text(encoding="utf-8"))
jsonschema.Draft202012Validator.check_schema(schema)
for path in BASE.rglob("*.yaml"):
    yaml.safe_load(path.read_text(encoding="utf-8"))
jsonschema.validate(yaml.safe_load((BASE / "assets/project-profile.yaml").read_text(encoding="utf-8")), schema)
with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    (root / "package.json").write_text(json.dumps({"scripts": {"test": "vitest run"}}), encoding="utf-8")
    (root / "Cargo.toml").write_text('[package]\nname="demo"\nversion="0.1.0"\n', encoding="utf-8")
    profile = scanner["scan"](root)
    serialized = scanner["to_yaml"](profile)
    assert yaml.safe_load(serialized) == profile
    jsonschema.validate(profile, schema)
rules = yaml.safe_load((BASE / "references/core-rules.yaml").read_text(encoding="utf-8"))["rules"]
assert len(rules) == 15 and len({rule["id"] for rule in rules}) == 15
assert all({"id", "title", "level", "rule", "rationale", "review", "enforcement"} <= rule.keys() for rule in rules)
print("YAML, schema, generated-profile roundtrip, and 15 Core Rules validated")
