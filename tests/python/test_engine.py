import asyncio
import json
import sqlite3
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from model_collaboration_engine import Engine
import pytest


def test_native_roundtrip(tmp_path, monkeypatch):
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            frames = [
                {"id": "mock", "choices": [{"index": 0, "delta": {"content": '{"greeting":"hello"}'}, "finish_reason": None}]},
                {"id": "mock", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 10, "completion_tokens": 5}},
            ]
            body = "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames) + "data: [DONE]\n\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            self.wfile.write(body.encode())

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("MODEL_API_KEY", "local-test-only")
    root = Path(__file__).resolve().parents[2]
    config = json.loads((root / "examples/config.json").read_text())
    config["database_path"] = str(tmp_path / "engine.db")
    config["models"][0]["base_url"] = f"http://127.0.0.1:{server.server_port}/v1"
    task = json.loads((root / "examples/task.json").read_text())
    task["deadline_ms"] = int(time.time() * 1000) + 10000

    async def run():
        async with await Engine.open(config) as engine:
            result = await engine.run(task)
            assert result["status"] == "completed"
            assert result["settled_cost"] == 15
            assert result["reserved_cost"] == 0
            assert json.loads(result["artifact"]["text"])["greeting"] == "hello"

    try:
        asyncio.run(run())
        assert len(requests) == 1
        assert requests[0]["stream"] is True
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_python_cancellation_finishes_accounting(tmp_path, monkeypatch):
    entered = threading.Event()
    release = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            self.rfile.read(int(self.headers["Content-Length"]))
            entered.set()
            release.wait(10)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("MODEL_API_KEY", "local-test-only")
    root = Path(__file__).resolve().parents[2]
    config = json.loads((root / "examples/config.json").read_text())
    config["database_path"] = str(tmp_path / "cancel.db")
    config["models"][0]["base_url"] = f"http://127.0.0.1:{server.server_port}/v1"
    task = json.loads((root / "examples/task.json").read_text())
    task["deadline_ms"] = int(time.time() * 1000) + 10000

    async def run():
        async with await Engine.open(config) as engine:
            assert await engine.recovery_records() == []
            pending = asyncio.create_task(engine.run(task))
            assert await asyncio.to_thread(entered.wait, 5)
            pending.cancel()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(pending, 2)
            records = await engine.recovery_records()
            assert len(records) == 1
            record = records[0]
            assert record["task"]["task_id"] == task["task_id"]
            assert record["status"] == "cancelled"
            assert record["ledger"]["calls"] == 1
            assert record["attempts"][0]["state"] == "unresolved"
            assert record["attempts"][0]["cost"] is None
            assert record["attempts"][0]["attempt_id"]
            assert record["attempts"][0]["amount"] == record["ledger"]["reserved"]
        with pytest.raises(RuntimeError, match="closing or closed"):
            await engine.recovery_records()
        async with await Engine.open(config) as reopened:
            assert await reopened.recovery_records() == records

    try:
        asyncio.run(run())
        with sqlite3.connect(config["database_path"]) as db:
            status, settled, reserved = db.execute("SELECT status,settled,reserved FROM tasks").fetchone()
            assert status == "cancelled"
            assert settled == 0
            assert reserved > 0
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        thread.join()
