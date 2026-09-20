#!/usr/bin/env python3
"""Detect root-level project tooling without executing project code (Python 3.11+)."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

if sys.version_info < (3, 11):
    raise SystemExit("scan_project.py requires Python 3.11 or newer")

import tomllib


class ScanError(ValueError):
    """An existing manifest could not be read or interpreted."""


def read_manifest(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    try:
        content = path.read_text(encoding="utf-8-sig")
        data = json.loads(content) if path.suffix == ".json" else tomllib.loads(content)
    except (OSError, UnicodeError, ValueError) as error:
        raise ScanError(f"Cannot read {path}: {error}") from error
    if not isinstance(data, dict):
        raise ScanError(f"Expected an object in {path}")
    return data


def mapping(data: dict[str, Any], key: str) -> dict[str, Any]:
    value = data.get(key, {})
    if not isinstance(value, dict):
        raise ScanError(f"Expected an object for {key}")
    return value


def js_tools(root: Path, package: dict[str, Any], quality: dict[str, list]) -> None:
    deps = set()
    for key in ("dependencies", "devDependencies", "peerDependencies"):
        deps.update(mapping(package, key))
    scripts = mapping(package, "scripts")
    if any(not isinstance(value, str) for value in scripts.values()):
        raise ScanError("package.json scripts must contain string commands")
    # Configuration presence is evidence; configuration code is never evaluated.
    patterns = {
        "eslint": ("eslint.config.*", ".eslintrc*"),
        "prettier": ("prettier.config.*", ".prettierrc*"),
        "vitest": ("vitest.config.*",),
        "jest": ("jest.config.*",),
        "@biomejs/biome": ("biome.json", "biome.jsonc"),
        "typescript": ("tsconfig*.json",),
    }
    for name, globs in patterns.items():
        if any(any(root.glob(pattern)) for pattern in globs):
            deps.add(name)
    manager = str(package.get("packageManager", "")).split("@", 1)[0]
    if manager not in {"npm", "pnpm", "yarn", "bun"}:
        manager = next((name for name, lock in (("pnpm", "pnpm-lock.yaml"),
                       ("yarn", "yarn.lock"), ("bun", "bun.lock"))
                       if (root / lock).exists()), "npm")
    tools = {
        "formatter": ("prettier", "@biomejs/biome"),
        "linter": ("eslint", "@biomejs/biome"),
        "typecheck": ("vue-tsc", "typescript"),
        "test_runner": ("vitest", "jest"),
    }
    aliases = {"typescript": "tsc", "@biomejs/biome": "biome"}
    script_keys = {
        "formatter": ("format:check", "format", "fmt"),
        "linter": ("lint", "check:lint"),
        "typecheck": ("typecheck", "type-check", "check:types"),
        "test_runner": ("test", "test:unit"),
    }
    for category, candidates in tools.items():
        for name in candidates:
            binary = aliases.get(name, name)
            matching = next((key for key in script_keys[category]
                             if re.search(r"(?<![\w-])" + re.escape(binary) + r"(?![\w-])",
                                          scripts.get(key, ""))), None)
            if name in deps or matching:
                quality[category].append({"name": binary,
                    "command": f"{manager} run {matching}" if matching else None,
                    "verification": "inspect", "scope": "javascript"})
        # Preserve custom scripts without pretending to know their implementation.
        key = next((key for key in script_keys[category] if key in scripts), None)
        if key and not any(item["command"] == f"{manager} run {key}" for item in quality[category]):
            quality[category].append({"name": f"script:{key}",
                "command": f"{manager} run {key}", "verification": "inspect", "scope": "javascript"})


def python_tools(pyproject: dict[str, Any], quality: dict[str, list]) -> None:
    config = mapping(pyproject, "tool")
    project = mapping(pyproject, "project")
    dependencies = project.get("dependencies", [])
    if not isinstance(dependencies, list):
        raise ScanError("project.dependencies must be an array")
    dependencies = list(dependencies)
    for group in mapping(project, "optional-dependencies").values():
        if not isinstance(group, list):
            raise ScanError("project.optional-dependencies groups must be arrays")
        dependencies.extend(group)
    for group in mapping(pyproject, "dependency-groups").values():
        if not isinstance(group, list):
            raise ScanError("dependency-groups entries must be arrays")
        dependencies.extend(group)
    names = {re.split(r"[\s\[<>=!~;]", dep, maxsplit=1)[0].lower() for dep in dependencies if isinstance(dep, str)}
    detected = names | set(config)

    def add(category: str, name: str, command: str) -> None:
        quality[category].append({"name": name, "command": command,
                                  "verification": "inspect", "scope": "python"})

    if "black" in detected:
        add("formatter", "black", "python -m black --check .")
    elif isinstance(config.get("ruff"), dict) and "format" in config["ruff"]:
        add("formatter", "ruff", "python -m ruff format --check .")
    for name in ("ruff", "flake8", "pylint"):
        if name in detected:
            add("linter", name, f"python -m {name} check ." if name == "ruff" else f"python -m {name} .")
    for name in ("mypy", "pyright"):
        if name in detected:
            add("typecheck", name, f"python -m {name} .")
    if "pytest" in detected:
        add("test_runner", "pytest", "python -m pytest")


def scan(root: Path) -> dict[str, Any]:
    root = root.resolve()
    if not root.is_dir():
        raise ScanError(f"Project root is not a directory: {root}")
    package = read_manifest(root / "package.json")
    pyproject = read_manifest(root / "pyproject.toml")
    read_manifest(root / "Cargo.toml")
    quality = {name: [] for name in ("formatter", "linter", "typecheck", "test_runner")}
    js_tools(root, package, quality)
    python_tools(pyproject, quality)
    deps = set(mapping(package, "dependencies")) | set(mapping(package, "devDependencies"))
    languages = []
    if (root / "package.json").exists() or any(root.glob("tsconfig*.json")):
        languages.append("typescript" if "typescript" in deps or any(root.glob("tsconfig*.json")) else "javascript")
    if (root / "pyproject.toml").exists():
        languages.append("python")
    if (root / "Cargo.toml").exists():
        languages.append("rust")
        for category, name, command in (
            ("formatter", "rustfmt", "cargo fmt --check"),
            ("linter", "clippy", "cargo clippy --all-targets"),
            ("typecheck", "cargo-check", "cargo check --all-targets"),
            ("test_runner", "cargo-test", "cargo test"),
        ):
            quality[category].append({"name": name, "command": command,
                                      "verification": "inspect", "scope": "rust"})
    frameworks = [name for name in ("vue", "react", "next") if name in deps]
    if "vite" in deps or any(root.glob("vite.config.*")):
        frameworks.append("vite")
    return {
        "standard": {"version": "0.2.0", "strictness": "balanced"},
        "project": {"languages": sorted(languages), "frameworks": sorted(frameworks),
            "architecture": {"style": "unknown",
                "source_roots": [name for name in ("src", "app", "lib") if (root / name).is_dir()],
                "test_roots": [name for name in ("tests", "test", "__tests__") if (root / name).is_dir()]}},
        "quality": quality,
        "agent": {"target": "codex", "instructions": []},
    }


def to_yaml(value: Any, indent: int = 0) -> str:
    """Emit JSON flow values, a YAML 1.2 subset, inside readable block mappings."""
    if isinstance(value, dict) and value:
        lines = []
        for key, item in value.items():
            prefix = " " * indent + key + ":"
            if isinstance(item, dict) and item:
                lines.extend((prefix, to_yaml(item, indent + 2)))
            else:
                lines.append(prefix + " " + json.dumps(item, ensure_ascii=False))
        return "\n".join(lines)
    return " " * indent + json.dumps(value, ensure_ascii=False)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", nargs="?", default=".", type=Path)
    parser.add_argument("--output", type=Path, help="Write profile; default is read-only stdout.")
    parser.add_argument("--force", action="store_true", help="Replace an existing profile.")
    args = parser.parse_args(argv)
    try:
        rendered = to_yaml(scan(args.root)) + "\n"
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            with args.output.open("w" if args.force else "x", encoding="utf-8") as stream:
                stream.write(rendered)
        else:
            print(rendered, end="")
    except (ScanError, OSError) as error:
        print(f"Scan failed: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
