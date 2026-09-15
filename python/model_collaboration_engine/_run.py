"""Host event-loop ownership and cleanup for one native execution."""
import asyncio
from contextvars import ContextVar
import inspect
import json
from typing import Any, Self

from ._types import ToolCallbacks

_tool_engine = ContextVar("model_collaboration_tool_engine", default=None)


class Run:
    """Use as an async context manager; consume events before awaiting result()."""

    def __init__(
        self, engine: Any, task: dict[str, Any], tools: ToolCallbacks | None,
        *, submission: bool = False
    ) -> None:
        self._engine = engine
        self._submission = submission
        self._task_json = json.dumps(task)
        self._tools = dict(tools or {})
        names = {spec["name"] for spec in task.get("tools", [])}
        missing = names - self._tools.keys()
        if missing:
            raise ValueError(f"Missing host tool callbacks: {', '.join(sorted(missing))}")
        if any(not callable(self._tools[name]) for name in names):
            raise TypeError("Host tool callbacks must be callable and return an awaitable")
        self._session = None
        self._outcome = None
        self._worker = None

    async def __aenter__(self) -> Self:
        if self._engine._closing:
            raise RuntimeError("Engine is closing or closed")
        if self._session is not None:
            raise RuntimeError("A Run cannot be entered twice")
        start = (
            self._engine._native.start_submission if self._submission
            else self._engine._native.start
        )
        self._session = start(self._task_json, bool(self._tools))
        self._worker = asyncio.create_task(self._serve_tools())
        self._outcome = asyncio.create_task(self._wait_result())
        self._engine._runs.add(self)
        return self

    async def _serve_tools(self) -> None:
        while True:
            raw = await self._session.next_tool()
            if raw is None:
                return
            request = json.loads(raw)
            execution_id = request["execution_id"]
            token = _tool_engine.set(self._engine)
            try:
                callback = self._tools[request["call"]["name"]]
                result = callback(request)
                if not inspect.isawaitable(result):
                    raise TypeError("Host tool callback must return an awaitable")
                result = await result
                self._session.reply(execution_id, json.dumps(result))
            except asyncio.CancelledError:
                self._session.fail_tool(execution_id, "Host tool callback cancelled; cost unknown")
                raise
            except Exception as exc:
                self._session.fail_tool(execution_id, f"{type(exc).__name__}: {exc}")
            finally:
                _tool_engine.reset(token)

    async def _wait_result(self) -> dict[str, Any]:
        try:
            return json.loads(await self._session.result())
        finally:
            self._worker.cancel()
            try:
                await asyncio.gather(self._worker, return_exceptions=True)
            finally:
                self._engine._runs.discard(self)

    def cancel(self) -> None:
        if self._session is not None:
            self._session.cancel()

    async def _drain(self) -> dict[str, Any]:
        # Preserve accounting even when the host issues repeated cancellation requests.
        interrupted = False
        while not self._outcome.done():
            try:
                await asyncio.shield(self._outcome)
            except asyncio.CancelledError:
                interrupted = True
                self.cancel()
            except Exception:
                break
        if interrupted:
            self._outcome.exception()  # Retrieve any native failure before propagating cancellation.
            raise asyncio.CancelledError
        return self._outcome.result()

    async def result(self) -> dict[str, Any]:
        if self._outcome is None:
            raise RuntimeError("Enter the Run context first")
        try:
            return await asyncio.shield(self._outcome)
        except asyncio.CancelledError:
            self.cancel()
            try:
                await self._drain()
            except Exception:
                pass
            raise

    def __aiter__(self) -> Self:
        return self

    async def __anext__(self) -> dict[str, Any]:
        if self._session is None:
            raise RuntimeError("Enter the Run context first")
        raw = await self._session.next_event()
        if raw is None:
            raise StopAsyncIteration
        return json.loads(raw)

    async def __aexit__(self, exc_type: type[BaseException] | None, *_: object) -> None:
        cancelled_here = not self._outcome.done()
        if cancelled_here:
            self.cancel()
        try:
            await self._drain()
        except Exception as error:
            if exc_type is None:
                if cancelled_here and isinstance(error, RuntimeError):
                    try:
                        if json.loads(str(error)).get("kind") == "cancelled":
                            return
                    except (ValueError, AttributeError):
                        pass
                raise
