"""Async JSON interface to the Rust model collaboration engine."""
import asyncio
import inspect
import json
import time
from typing import Any, Self

from ._configuration import Configuration, parse_config
from ._configuration import load_config as load_config
from ._native import Engine as _Engine
from ._run import Run, _tool_engine
from ._types import EventCallback, ToolCallbacks


class Engine:
    def __init__(self, native: _Engine) -> None:
        self._native = native
        self._closing = False
        self._close_task = None
        self._runs = set()

    @classmethod
    async def open(cls, config: Configuration | dict[str, Any]) -> Self:
        """Open a resolved configuration or a programmatic configuration dictionary.

        Layered dictionaries use parse_config with the process directory as base.
        Existing flat dictionaries remain the low-level component configuration.
        """
        if isinstance(config, dict) and "schema_version" in config:
            config = parse_config(config)
        if isinstance(config, Configuration):
            return cls(await _Engine.open_resolved(config))
        return cls(await _Engine.open(json.dumps(config)))

    def stream(self, task: dict[str, Any], *, tools: ToolCallbacks | None = None) -> Run:
        """Stream one submission's optional planning and bounded execution."""
        if self._closing:
            raise RuntimeError("Engine is closing or closed")
        return Run(self, task, tools)

    async def run(
        self,
        task: dict[str, Any],
        *,
        tools: ToolCallbacks | None = None,
        on_event: EventCallback | None = None,
    ) -> dict[str, Any]:
        """Run a submission; omitted planning means disabled and requires a strategy."""
        return await self._consume(self.stream(task, tools=tools), task, on_event)

    async def _consume(
        self, stream: Run, task: dict[str, Any], on_event: EventCallback | None
    ) -> dict[str, Any]:
        async with stream as run:
            async for event in run:
                if on_event is not None:
                    result = on_event(event)
                    if inspect.isawaitable(result):
                        remaining = (task["deadline_ms"] - task["finalization_ms"]) / 1000 - time.time()
                        async with asyncio.timeout(max(0, remaining)):
                            await result
            return await run.result()

    async def _finish_close(self) -> None:
        await self._native.close()
        outcomes = [run._outcome for run in tuple(self._runs)]
        if outcomes:
            await asyncio.gather(*outcomes, return_exceptions=True)

    async def recovery_records(self) -> list[dict[str, Any]]:
        """Inspect persisted tasks and attempt evidence without replaying calls."""
        if self._closing:
            raise RuntimeError("Engine is closing or closed")
        return json.loads(await self._native.recovery_records())

    async def record_feedback(self, feedback: dict[str, Any]) -> None:
        """Save terminal-artifact feedback; identical feedback IDs are idempotent."""
        if self._closing:
            raise RuntimeError("Engine is closing or closed")
        await self._native.record_feedback(json.dumps(feedback))

    async def evaluations(self, task_id: str) -> list[dict[str, Any]]:
        """Read candidate artifacts and separate deterministic/critic evidence."""
        if self._closing:
            raise RuntimeError("Engine is closing or closed")
        return json.loads(await self._native.evaluations(task_id))

    async def metrics(self) -> dict[str, Any]:
        """Read an aggregate snapshot, without inferring acceptance."""
        if self._closing:
            raise RuntimeError("Engine is closing or closed")
        return json.loads(await self._native.metrics())

    async def close(self) -> None:
        if _tool_engine.get() is self:
            raise RuntimeError("Close the engine outside its tool callback to avoid waiting on itself")
        self._closing = True
        if self._close_task is None or (
            self._close_task.done() and self._close_task.exception() is not None
        ):
            self._close_task = asyncio.create_task(self._finish_close())
        interrupted = False
        while not self._close_task.done():
            try:
                await asyncio.shield(self._close_task)
            except asyncio.CancelledError:
                interrupted = True
            except Exception:
                break
        if interrupted:
            self._close_task.exception()
            raise asyncio.CancelledError
        return self._close_task.result()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.close()
