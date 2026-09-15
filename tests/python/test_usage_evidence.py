"""Usage survives protocol failures and cancellation after it has been received."""
import asyncio
import json
import sqlite3
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from model_collaboration_engine import Engine


@pytest.fixture(params=["chat_completions", "responses"])
def evidence_server(request, tmp_path, monkeypatch):
    endpoint = request.param
    mode = {"failure": "arguments", "usage": True, "tokens": 10, "requests": 0}
    release = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            self.rfile.read(int(self.headers["Content-Length"]))
            mode["requests"] += 1
            failure = mode["failure"]
            tool = {"id": "call", "name": "lookup", "arguments": "{}"}
            if failure in tool:
                tool[failure] = "{broken" if failure == "arguments" else ""
            if endpoint == "chat_completions":
                delta = {"content": "received"} if failure == "cancel" else {
                    "tool_calls": [{"index": 0, "id": tool["id"], "function": {
                        "name": tool["name"], "arguments": tool["arguments"],
                    }}],
                }
                frame = {"id": "provider-evidence", "choices": [{
                    "index": 0, "delta": delta, "finish_reason": "tool_calls",
                }]}
                if mode["usage"]:
                    frame["usage"] = {"prompt_tokens": mode["tokens"], "completion_tokens": 5}
                frames = [frame]
            else:
                response = {"id": "provider-evidence", "status": "completed", "output": [{
                    "type": "function_call", "call_id": tool["id"],
                    "name": tool["name"], "arguments": tool["arguments"],
                }]}
                if mode["usage"]:
                    response["usage"] = {"input_tokens": mode["tokens"], "output_tokens": 5}
                frames = [{"type": "response.completed", "response": response}]
                if failure == "cancel":
                    frames.append({"type": "response.output_text.delta", "delta": "received"})
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            body = "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames)
            if failure == "trailing":
                body += "data: {broken\n\n"
            if failure != "cancel":
                body += "data: [DONE]\n\n"
            self.wfile.write(body.encode())
            self.wfile.flush()
            if failure == "cancel":
                release.wait(10)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("MODEL_API_KEY", "local-test-placeholder")
    root = Path(__file__).resolve().parents[2]
    config = json.loads((root / "examples/config.json").read_text())
    config["database_path"] = str(tmp_path / "engine.db")
    config["models"][0].update(
        endpoint=endpoint, base_url=f"http://127.0.0.1:{server.server_port}/v1",
    )
    task = json.loads((root / "examples/task.json").read_text())
    task["deadline_ms"] = int(time.time() * 1000) + 15_000
    try:
        yield config, task, mode
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        thread.join()


@pytest.mark.parametrize("failure", ["arguments", "id", "name", "trailing"])
@pytest.mark.parametrize("with_usage", [True, False])
def test_protocol_failure_settles_received_usage(evidence_server, failure, with_usage):
    config, task, mode = evidence_server
    mode.update(failure=failure, usage=with_usage)
    _run_failure(config, task, "protocol")
    assert mode["requests"] == 1
    _assert_evidence(config, "failed", 15 if with_usage else None)


def test_protocol_failure_records_overrun_before_budget_error(evidence_server):
    config, task, mode = evidence_server
    mode["tokens"] = 50_000
    _run_failure(config, task, "budget")
    assert mode["requests"] == 1
    _assert_evidence(config, "failed", 50_005)


def test_cancellation_after_usage_keeps_known_cost(evidence_server):
    config, task, mode = evidence_server
    mode["failure"] = "cancel"

    async def run():
        async with await Engine.open(config) as engine:
            async with engine.stream(task) as stream:
                async for event in stream:
                    if event["kind"] == "content_delta":
                        stream.cancel()
                        break

    asyncio.run(run())
    assert mode["requests"] == 1
    _assert_evidence(config, "cancelled", 15)


def test_timeout_after_usage_keeps_known_cost(evidence_server):
    config, task, mode = evidence_server
    mode["failure"] = "cancel"  # Keep the transport open after the usage-bearing frame.
    task["deadline_ms"] = int(time.time() * 1000) + 2500
    task["finalization_ms"] = 100
    received = []

    async def run():
        async def observe(event):
            if event["kind"] == "content_delta":
                received.append(event)

        async with await Engine.open(config) as engine:
            with pytest.raises(RuntimeError) as caught:
                await engine.run(task, on_event=observe)
            assert json.loads(str(caught.value))["kind"] == "deadline"

    asyncio.run(run())
    assert received  # Proves the adapter received usage before its deadline elapsed.
    assert mode["requests"] == 1
    _assert_evidence(config, "failed", 15, call_status="timed_out")


def _run_failure(config, task, kind):
    async def run():
        async with await Engine.open(config) as engine:
            with pytest.raises(RuntimeError) as caught:
                await engine.run(task)
            assert json.loads(str(caught.value))["kind"] == kind

    asyncio.run(run())


def _assert_evidence(config, status, cost, call_status=None):
    with sqlite3.connect(config["database_path"]) as db:
        state, settled, reserved, calls = db.execute(
            "SELECT status,settled,reserved,calls FROM tasks"
        ).fetchone()
        amount, actual, outcome = db.execute("SELECT amount,cost,outcome FROM attempts").fetchone()
    assert state == status
    assert calls == 1
    assert actual == cost
    assert settled == (cost or 0)
    assert reserved == (amount if cost is None else 0)
    outcome = json.loads(outcome)
    assert outcome["request_id"] == "provider-evidence"
    assert outcome["call_metrics"]["status"] == (call_status or status)
