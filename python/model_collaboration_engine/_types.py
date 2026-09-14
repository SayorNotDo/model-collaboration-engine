"""Callback contracts shared by the Python entry point and run lifecycle."""
from collections.abc import Awaitable, Callable, Mapping
from typing import Any

ToolCallback = Callable[[dict[str, Any]], Awaitable[dict[str, Any]]]
ToolCallbacks = Mapping[str, ToolCallback]
EventCallback = Callable[[dict[str, Any]], Awaitable[None] | None]
