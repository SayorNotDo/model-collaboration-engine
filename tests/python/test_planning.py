"""Planning through real HTTP/SSE and the installed native Python boundary."""
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
def planning_server(request, tmp_path, monkeypatch):
    endpoint = request.param
    proposal = {
        "proposal_version": 1, "task_type": "writing", "classification_confidence": 0.8,
        "strategy": "single", "required_capabilities": ["text"],
        "suggested_acceptance": {
            "version": "1", "nonempty": False, "required_substrings": [], "json_object": False,
        }, "reason": "Simple greeting",
    }
    mode = {
        "proposal": proposal, "usage": True, "block": False, "tool": False,
        "entered": threading.Event(), "release": threading.Event(), "requests": [],
    }

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            messages = payload.get("messages", payload.get("input"))
            planner = messages[0]["content"].startswith("Classify the host task")
            mode["requests"].append((planner, payload))
            if planner:
                mode["entered"].set()
                if mode["block"]:
                    mode["release"].wait(5)
                text = mode["proposal"]
                if not isinstance(text, str):
                    text = json.dumps(text)
            else:
                text = '{"greeting":"hello"}'
            with_usage = not planner or mode["usage"]
            if endpoint == "chat_completions":
                delta = {"content": text}
                if planner and mode["tool"]:
                    delta = {"tool_calls": [{
                        "index": 0, "id": "forbidden", "type": "function",
                        "function": {"name": "lookup", "arguments": "{}"},
                    }]}
                frames = [{"id": "mock", "choices": [{
                    "index": 0, "delta": delta, "finish_reason": "stop",
                }]}]
                if with_usage:
                    frames[0]["usage"] = {"prompt_tokens": 10, "completion_tokens": 5}
            else:
                output = []
                if planner and mode["tool"]:
                    output = [{"type": "function_call", "call_id": "forbidden",
                               "name": "lookup", "arguments": "{}"}]
                completed = {"id": "mock", "status": "completed", "output": output}
                if with_usage:
                    completed["usage"] = {"input_tokens": 10, "output_tokens": 5}
                frames = [
                    {"type": "response.output_text.delta", "delta": text},
                    {"type": "response.completed", "response": completed},
                ]
            body = "".join("data: " + json.dumps(f) + "\n\n" for f in frames)
            body += "data: [DONE]\n\n"
            try:
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(body.encode())
            except (BrokenPipeError, ConnectionResetError):
                pass  # Cancellation intentionally disconnects the native client.

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("MODEL_API_KEY", "local-test-only")
    root = Path(__file__).resolve().parents[2]
    config = json.loads((root / "examples/config.json").read_text())
    config["database_path"] = str(tmp_path / "engine.db")
    config["planner_models"] = ["local"]
    config["close_grace_ms"] = 0
    config["models"][0]["endpoint"] = endpoint
    config["models"][0]["base_url"] = f"http://127.0.0.1:{server.server_port}/v1"
    submission = json.loads((root / "examples/task.json").read_text())
    submission.pop("strategy")
    submission.update({
        "schema_version": 1, "deadline_ms": int(time.time() * 1000) + 15000,
        "planning": {"mode": "auto", "max_cost": 20000, "timeout_ms": 5000},
    })
    try:
        yield config, submission, mode
    finally:
        mode["release"].set()
        server.shutdown()
        server.server_close()
        thread.join()


def test_submit_stream_hides_plan_json_and_preserves_acceptance(planning_server):
    config, submission, mode = planning_server
    events = []

    async def run():
        async with await Engine.open(config) as engine:
            async with engine.stream(submission) as stream:
                async for event in stream:
                    events.append(event)
                result = await stream.result()
                assert result["status"] == "completed"
                assert result["settled_cost"] == 30
        assert [e["sequence"] for e in events] == list(range(1, len(events) + 1))
        assert {e["node_id"] for e in events if e["kind"] == "content_delta"} == {"invoke"}
        assert "planning_completed" in [e["kind"] for e in events]
        assert "plan_validated" in [e["kind"] for e in events]

    asyncio.run(run())
    assert len(mode["requests"]) == 2
    for _, payload in mode["requests"]:
        assert "tools" not in payload
        assert payload.get("response_format", payload.get("text", {}).get("format")) == {
            "type": "json_object",
        }
    with sqlite3.connect(config["database_path"]) as db:
        spec, plan, calls = db.execute("SELECT spec,plan,calls FROM tasks").fetchone()
        assert calls == 2
        assert json.loads(spec)["submission"]["goal"] == submission["goal"]
        plan = json.loads(plan)["effective_plan"]
        assert plan["acceptance"]["nonempty"] and plan["acceptance"]["json_object"]


@pytest.mark.parametrize("invalid", [False, True])
def test_unknown_usage_preserved_through_success_or_explicit_fallback(planning_server, invalid):
    config, submission, mode = planning_server
    mode["usage"] = False
    if invalid:
        mode["proposal"] = "not json"
        submission["planning"]["fallback"] = {"task_type": "general", "strategy": "single"}
    events = []

    async def run():
        async with await Engine.open(config) as engine:
            result = await engine.run(submission, on_event=events.append)
            assert result["settled_cost"] == 15
            assert result["reserved_cost"] > 0
            records = await engine.recovery_records()
            assert records[0]["submission"]["schema_version"] == 1
            assert records[0]["attempts"][0]["cost"] is None
            assert records[0]["plan"]["effective_plan"]["plan_version"] == 1
        async with await Engine.open(config) as reopened:
            assert await reopened.recovery_records() == records

    asyncio.run(run())
    assert len(mode["requests"]) == 2
    assert ("planning_fallback" in [e["kind"] for e in events]) == invalid


@pytest.mark.parametrize("violation", ["json", "unknown_field", "tool"])
def test_invalid_planner_output_settles_without_execution(planning_server, violation):
    config, submission, mode = planning_server
    if violation == "json":
        mode["proposal"] = "not json"
    elif violation == "unknown_field":
        mode["proposal"]["budget"] = 999999
    else:
        mode["tool"] = True

    async def run():
        async with await Engine.open(config) as engine:
            with pytest.raises(RuntimeError, match="plan_validation"):
                await engine.run(submission)

    asyncio.run(run())
    assert len(mode["requests"]) == 1
    with sqlite3.connect(config["database_path"]) as db:
        status, calls, settled, reserved = db.execute(
            "SELECT status,calls,settled,reserved FROM tasks"
        ).fetchone()
        assert (status, calls, settled, reserved) == ("failed", 1, 15, 0)


@pytest.mark.parametrize("stop", ["cancel", "close", "early_exit"])
def test_planning_cancellation_and_shutdown_wait_for_accounting(planning_server, stop):
    config, submission, mode = planning_server
    mode["block"] = True
    submission["planning"]["fallback"] = {"task_type": "general", "strategy": "single"}

    async def run():
        async with await Engine.open(config) as engine:
            if stop == "early_exit":
                async with engine.stream(submission):
                    assert await asyncio.to_thread(mode["entered"].wait, 3)
            else:
                pending = asyncio.create_task(engine.run(submission))
                assert await asyncio.to_thread(mode["entered"].wait, 3)
                if stop == "cancel":
                    pending.cancel()
                    pending.cancel()
                    with pytest.raises(asyncio.CancelledError):
                        await asyncio.wait_for(pending, 3)
                else:
                    await engine.close()
                    with pytest.raises(RuntimeError, match="cancelled"):
                        await pending
        async with await Engine.open(config) as reopened:
            record, = await reopened.recovery_records()
            assert record["status"] == "cancelled"
            assert record["ledger"]["calls"] == 1
            assert record["attempts"][0]["state"] == "unresolved"
            assert record["plan"]["planning"]["error"]["kind"] == "cancelled"

    asyncio.run(run())
    assert len(mode["requests"]) == 1


@pytest.mark.parametrize("mode_name", ["disabled", "auto", "required"])
def test_explicit_host_choices_control_planning_mode(planning_server, mode_name):
    config, submission, mode = planning_server
    submission.update({"strategy": "single", "task_type": "writing"})
    submission["planning"]["mode"] = mode_name

    async def run():
        async with await Engine.open(config) as engine:
            assert (await engine.run(submission))["status"] == "completed"

    asyncio.run(run())
    assert len(mode["requests"]) == (2 if mode_name == "required" else 1)


def test_submission_missing_callback_rejected_before_planning(planning_server):
    config, submission, mode = planning_server
    submission["tools"] = [{"name": "lookup", "description": "test", "parameters": {}, "max_cost": 0}]

    async def run():
        async with await Engine.open(config) as engine:
            with pytest.raises(ValueError, match="Missing host tool callbacks"):
                await engine.run(submission)
    asyncio.run(run())
    assert mode["requests"] == []
