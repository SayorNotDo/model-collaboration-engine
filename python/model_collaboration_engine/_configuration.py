"""Explicit, synchronous loading of bounded JSON configuration files."""
import json
from os import PathLike, fspath
from typing import Any

from ._native import Configuration


def load_config(path: str | PathLike[str]) -> Configuration:
    """Load schema 1 JSON; database paths are relative to its directory.

    Validates without reading credentials or opening external resources.
    Raises RuntimeError with structured configuration errors and field paths.
    """
    return Configuration.load(fspath(path))


def parse_config(
    document: dict[str, Any], *, base_dir: str | PathLike[str] = "."
) -> Configuration:
    """Validate a detached, read-only snapshot using an explicit path base.

    No environment interpolation, implicit merge, dotenv loading or hot reload.
    """
    return Configuration.parse(json.dumps(document, allow_nan=False), fspath(base_dir))
