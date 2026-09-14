import asyncio
import json
import sqlite3
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
from model_collaboration_engine import Engine


@pytest.fixture
def model_server(tmp_path, monkeypatch):
    requests = []
    mode = {"endpoint": "chat_completions", "repeat": False, "pause_final": False, "release": threading.Event()}

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(request)
            tool = len(requests) == 1 or mode["repeat"]
            if mode["endpoint"] == "chat_completions":
                delta = {"tool_calls": [{"index": 0, "id": "call-1", "type": "function", "function": {"name": "lookup", "arguments": '{"query":"x"}'}}]} if tool else {"content": '{"greeting":"hello"}'}
                frames = [{"id": "mock", "choices": [{"index": 0, "delta": delta, "finish_reason": "tool_calls" if tool else "stop"}], "usage": {"prompt_tokens": 10, "completion_tokens": 5}}]
            else:
                frames = [] if tool else [{"type": "response.output_text.delta", "delta": '{"greeting":"hello"}'}]
                output = [{"type": "function_call", "call_id": "call-1", "name": "lookup", "arguments": '{"query":"x"}'}] if tool else []
                frames.append({"type": "response.completed", "response": {"id": "mock", "status": "completed", "output": output, "usage": {"input_tokens": 10, "output_tokens": 5}}})
            body = "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames) + "data: [DONE]\n\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            if mode["pause_final"] and not tool:
                self.wfile.write(body.removesuffix("data: [DONE]\n\n").encode())
                self.wfile.flush()
                mode["release"].wait(5)
                self.wfile.write(b"data: [DONE]\n\n")
            else:
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
    config["models"][0]["capabilities"].append("tools")
    task = json.loads((root / "examples/task.json").read_text())
    task["deadline_ms"] = int(time.time() * 1000) + 10000
    task["tools"] = [{"name": "lookup", "description": "Local test lookup", "parameters": {"type": "object"}, "max_cost": 20}]
    try:
        yield config, task, requests, mode
    finally:
        mode["release"].set()
        server.shutdown()
        server.server_close()
        thread.join()


@pytest.mark.parametrize("endpoint", ["chat_completions", "responses"])
def test_tool_roundtrip_streams_and_uses_host_loop(model_server, endpoint):
    config, task, requests, mode = model_server
    config["models"][0]["endpoint"] = mode["endpoint"] = endpoint
    events, calls = [], []

    async def run():
        loop = asyncio.get_running_loop()

        async def lookup(request):
            assert asyncio.get_running_loop() is loop
            assert request["call"]["arguments"] == {"query": "x"}
            assert request["max_cost"] == 20
            calls.append(request)
            return {"output": "lookup value", "actual_cost": 3}

        async with await Engine.open(config) as engine:
            async with engine.stream(task, tools={"lookup": lookup}) as stream:
                async for event in stream:
                    events.append(event)
                result = await stream.result()
                assert result["settled_cost"] == 33
                assert result["reserved_cost"] == 0
                assert result["status"] == "completed"

    asyncio.run(run())
    assert len(requests) == 2
    assert len(calls) == 1
    if endpoint == "chat_completions":
        assert requests[1]["messages"][-1] == {"role": "tool", "content": "lookup value", "tool_call_id": "call-1"}
    else:
        assert requests[1]["input"][-1] == {"type": "function_call_output", "call_id": "call-1", "output": "lookup value"}
    kinds = [event["kind"] for event in events]
    assert "tool_requested" in kinds and "tool_completed" in kinds and "content_delta" in kinds
    assert [event["sequence"] for event in events] == list(range(1, len(events) + 1))


def test_missing_callback_rejected_before_dispatch(model_server):
    config, task, requests, _ = model_server

    async def run():
        async with await Engine.open(config) as engine:
            with pytest.raises(ValueError, match="Missing host tool"):
                await engine.run(task)

    asyncio.run(run())
    assert requests == []


@pytest.mark.parametrize("bad_result", [False, True])
def test_host_failure_preserves_unknown_cost_without_retry(model_server, bad_result):
    config, task, requests, _ = model_server

    async def lookup(_):
        if bad_result:
            return {"output": "value", "actual_cost": -1}
        raise ValueError("host denied request")

    async def run():
        async with await Engine.open(config) as engine:
            with pytest.raises(RuntimeError) as caught:
                await engine.run(task, tools={"lookup": lookup})
            assert json.loads(str(caught.value))["kind"] == "tool"

    asyncio.run(run())
    assert len(requests) == 1
    with sqlite3.connect(config["database_path"]) as db:
        assert db.execute("SELECT status,settled,reserved,calls FROM tasks").fetchone() == ("failed", 15, 20, 2)


def test_duplicate_tool_id_is_not_executed_twice(model_server):
    config, task, requests, mode = model_server
    mode["repeat"] = True
    calls = []

    async def lookup(request):
        calls.append(request)
        return {"output": "value", "actual_cost": 3}

    async def run():
        async with await Engine.open(config) as engine:
            with pytest.raises(RuntimeError) as caught:
                await engine.run(task, tools={"lookup": lookup})
            assert json.loads(str(caught.value))["kind"] == "protocol"

    asyncio.run(run())
    assert len(requests) == 2 and len(calls) == 1


@pytest.mark.parametrize("stop", ["cancel", "deadline", "repeated_cancel"])
def test_stopped_host_callback_cleans_up_and_preserves_reservation(model_server, stop):
    config, task, requests, _ = model_server
    if stop == "deadline":
        task["deadline_ms"] = int(time.time() * 1000) + 1500
        task["finalization_ms"] = 100

    async def run():
        entered, cleaned = asyncio.Event(), asyncio.Event()

        async def lookup(_):
            entered.set()
            try:
                await asyncio.Future()
            finally:
                if stop == "repeated_cancel":
                    await asyncio.sleep(0.1)
                cleaned.set()

        async with await Engine.open(config) as engine:
            pending = asyncio.create_task(engine.run(task, tools={"lookup": lookup}))
            await asyncio.wait_for(entered.wait(), 2)
            if stop in ("cancel", "repeated_cancel"):
                pending.cancel()
                if stop == "repeated_cancel":
                    await asyncio.sleep(0.02)
                    pending.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await asyncio.wait_for(pending, 2)
            else:
                with pytest.raises(RuntimeError) as caught:
                    await asyncio.wait_for(pending, 3)
                assert json.loads(str(caught.value))["kind"] == "deadline"
            assert cleaned.is_set()

    asyncio.run(run())
    assert len(requests) == 1
    with sqlite3.connect(config["database_path"]) as db:
        assert db.execute("SELECT settled,reserved FROM tasks").fetchone() == (15, 20)


def test_early_stream_exit_cancels_and_can_close_engine(model_server):
    config, task, _, _ = model_server
    config["event_capacity"] = 1

    async def lookup(_):
        await asyncio.Future()

    async def run():
        async with await Engine.open(config) as engine:
            async with engine.stream(task, tools={"lookup": lookup}) as stream:
                async for event in stream:
                    assert event["kind"] == "task_started"
                    break

    asyncio.run(run())


def test_on_event_receives_delta_before_http_response_finishes(model_server):
    config, task, _, mode = model_server
    mode["pause_final"] = True
    events = []

    async def lookup(_):
        return {"output": "value", "actual_cost": 0}

    async def observe(event):
        events.append(event)
        if event["kind"] == "content_delta":
            mode["release"].set()

    async def run():
        async with await Engine.open(config) as engine:
            result = await asyncio.wait_for(engine.run(task, tools={"lookup": lookup}, on_event=observe), 3)
            assert result["status"] == "completed"

    asyncio.run(run())
    assert mode["release"].is_set()
    assert any(e["kind"] == "content_delta" for e in events)


def test_stalled_event_callback_has_a_deadline(model_server):
    config, task, _, _ = model_server
    config["event_capacity"] = 1
    task["deadline_ms"] = int(time.time() * 1000) + 1500
    task["finalization_ms"] = 100

    async def lookup(_):
        return {"output": "value", "actual_cost": 0}

    async def observe(_):
        await asyncio.Future()

    async def run():
        async with await Engine.open(config) as engine:
            with pytest.raises(TimeoutError):
                await engine.run(task, tools={"lookup": lookup}, on_event=observe)

    asyncio.run(run())


@pytest.mark.parametrize("cancel_close", [False, True])
def test_close_drains_active_tool_and_releases_database(model_server, cancel_close, monkeypatch):
    config, task, _, _ = model_server
    config["close_grace_ms"] = 100
    config["cleanup_timeout_ms"] = 1000

    async def run():
        entered, cleaned = asyncio.Event(), asyncio.Event()
        cleanup_started, release_cleanup = asyncio.Event(), asyncio.Event()
        close_waits = asyncio.Queue()

        async def lookup(_):
            entered.set()
            try:
                await asyncio.Future()
            finally:
                cleanup_started.set()
                await asyncio.wait_for(release_cleanup.wait(), 2)
                cleaned.set()

        engine = await Engine.open(config)
        original_shield = asyncio.shield

        def observe_close_wait(future):
            # A signal for each protected wait proves cancellation was handled
            # before the next cancellation is sent.
            if future is engine._close_task:
                close_waits.put_nowait(None)
            return original_shield(future)

        monkeypatch.setattr(asyncio, "shield", observe_close_wait)
        stale = engine.stream(task, tools={"lookup": lookup})
        pending = asyncio.create_task(engine.run(task, tools={"lookup": lookup}))
        await asyncio.wait_for(entered.wait(), 2)
        closing = asyncio.create_task(engine.close())
        await asyncio.wait_for(cleanup_started.wait(), 2)
        await asyncio.wait_for(close_waits.get(), 2)
        if cancel_close:
            closing.cancel()
            await asyncio.wait_for(close_waits.get(), 2)
            closing.cancel()
            await asyncio.wait_for(close_waits.get(), 2)
            assert not closing.done()
            assert not cleaned.is_set()
            release_cleanup.set()
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(closing, 2)
        else:
            release_cleanup.set()
            await asyncio.wait_for(closing, 2)
        assert cleaned.is_set()
        with pytest.raises(RuntimeError) as caught:
            await pending
        assert json.loads(str(caught.value))["kind"] == "cancelled"
        with pytest.raises(RuntimeError, match="closing or closed"):
            async with stale:
                pass
        await asyncio.gather(engine.close(), engine.close())
        reopened = await Engine.open(config)
        await reopened.close()

    asyncio.run(run())
    with sqlite3.connect(config["database_path"]) as db:
        assert db.execute("SELECT status,settled,reserved FROM tasks").fetchone() == ("cancelled", 15, 20)


def test_close_inside_tool_is_rejected_without_deadlock(model_server):
    config, task, _, _ = model_server

    async def run():
        async with await Engine.open(config) as engine:
            async def lookup(_):
                await engine.close()

            with pytest.raises(RuntimeError) as caught:
                await asyncio.wait_for(engine.run(task, tools={"lookup": lookup}), 2)
            assert json.loads(str(caught.value))["kind"] == "tool"

    asyncio.run(run())
