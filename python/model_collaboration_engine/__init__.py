"""Async JSON interface to the Rust model collaboration engine."""
import asyncio
import json
from ._native import Engine as _Engine
from ._native import Cancellation


class Engine:
    def __init__(self, native):
        self._native = native

    @classmethod
    async def open(cls, config: dict):
        return cls(await _Engine.open(json.dumps(config)))

    async def run(self, task: dict) -> dict:
        # Keep the Rust future alive through Python cancellation so accounting finishes.
        cancellation = Cancellation()
        pending = asyncio.ensure_future(self._native.run(json.dumps(task), cancellation))
        try:
            return json.loads(await asyncio.shield(pending))
        except asyncio.CancelledError:
            cancellation.cancel()
            try:
                await asyncio.shield(pending)
            except RuntimeError:
                pass
            raise

    async def close(self):
        await self._native.close()

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_):
        await self.close()
