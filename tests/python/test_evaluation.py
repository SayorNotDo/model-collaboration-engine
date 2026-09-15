"""Controlled evaluations through the public Engine and local HTTP transport."""
import asyncio
import importlib
import json
import sqlite3
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
from test_configuration import document

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture
def evaluation(tmp_path, monkeypatch):
    monkeypatch.syspath_prepend(str(ROOT / "examples"))
    runner = importlib.import_module("evaluation.runner")
    mode = {"usage": True, "text": '{"name":"Ada","count":2}', "requests": [], "tokens": 10,
            "block": False, "entered": threading.Event(), "release": threading.Event()}
    output = tmp_path / "results"

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            revisions = []
            for path in output.glob("*.db"):
                with sqlite3.connect(path) as db:
                    revisions.append(db.execute("SELECT COUNT(*) FROM feedback").fetchone()[0])
            mode["requests"].append((request, revisions))
            mode["entered"].set()
            if mode.get("http_error") and len(mode["requests"]) > 1:
                self.send_response(500)
                self.end_headers()
                return
            if mode["block"]:
                mode["release"].wait(10)
            text = mode["text"]
            if mode.get("after_smoke") and len(mode["requests"]) > 1:
                text = mode["after_smoke"]
            frame = {"id": "mock", "choices": [{"index": 0,
                     "delta": {"content": text}, "finish_reason": "stop"}]}
            if mode["usage"]:
                frame["usage"] = {"prompt_tokens": mode["tokens"], "completion_tokens": 5}
            try:
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(("data: " + json.dumps(frame) + "\n\ndata: [DONE]\n\n").encode())
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("MODEL_API_KEY", "private-test-value")
    config = document()
    config["providers"][0]["base_url"] = f"http://127.0.0.1:{server.server_port}/v1"
    config["providers"][0]["models"][0].update(model="mock-extractor", price_version="test-v1")
    config["providers"][0]["models"].append(config["providers"][0]["models"][0] | {"id": "second", "acceptance": 0.9})
    config_path = tmp_path / "config.json"
    config_path.write_text(json.dumps(config))
    suite = {"version": "synthetic-v1", "budget": 100000, "max_call_cost": 20000,
             "output_tokens": 256, "timeout_ms": 15000, "max_calls": 4,
             "max_rounds": 2, "max_attempts": 2,
             "cases": [{"id": str(i), "input": "Ada ordered 2 items.",
                        "expected": {"name": "Ada", "count": 2}} for i in range(2)]}
    suite_path = tmp_path / "suite.json"
    suite_path.write_text(json.dumps(suite))
    try:
        yield runner, config_path, suite_path, output, mode
    finally:
        mode["release"].set()
        server.shutdown()
        server.server_close()
        thread.join()


def test_paired_run_defers_feedback_and_exports_evidence(evaluation):
    runner, config, suite, output, mode = evaluation
    report = asyncio.run(runner.evaluate(config, suite, output, 500000))
    assert report["status"] == "completed"
    assert len(mode["requests"]) == 5
    assert all(not any(revisions) for _, revisions in mode["requests"])
    assert [r["group"] for r in report["runs"]] == [
        "smoke", "single", "cascade", "cascade", "single",
    ]
    for group in ("single", "cascade"):
        summary = report["groups"][group]
        assert summary["correct"] == 2
        assert summary["planned"] == 2
        assert summary["metrics"]["known_cost"] == 30
        assert summary["metrics"]["feedback_revision"] == 2
        assert summary["mean_task_elapsed_ms"] >= 0
    for request, _ in mode["requests"]:
        assert '"expected"' not in json.dumps(request)
    assert "private-test-value" not in (output / "manifest.json").read_text()
    assert len(list(output.glob("*.db"))) == 3


@pytest.mark.parametrize("condition", ["budget", "exists", "credentials"])
def test_preflight_refuses_before_dispatch(evaluation, condition, monkeypatch):
    runner, config, suite, output, mode = evaluation
    budget = 500000
    if condition == "budget":
        budget -= 1
    elif condition == "exists":
        output.mkdir()
        (output / "keep").write_text("keep")
    else:
        monkeypatch.delenv("MODEL_API_KEY")
    with pytest.raises((ValueError, FileExistsError)):
        asyncio.run(runner.evaluate(config, suite, output, budget))
    assert mode["requests"] == []
    if condition == "exists":
        assert (output / "keep").read_text() == "keep"


def test_unknown_cost_stops_and_marks_unexecuted(evaluation):
    runner, config, suite, output, mode = evaluation
    mode["usage"] = False
    report = asyncio.run(runner.evaluate(config, suite, output, 500000))
    assert report["status"] == "stopped"
    assert report["stop_reason"] == "unknown_cost"
    assert len(mode["requests"]) == 1
    assert all(row["status"] == "not_executed" for row in report["runs"][1:])
    assert report["groups"]["single"]["not_executed"] == 2
    assert report["groups"]["smoke"]["metrics"]["unknown_cost_attempts"] == 1


def test_cancellation_preserves_database_and_report(evaluation):
    runner, config, suite, output, mode = evaluation
    mode["block"] = True

    async def run():
        task = asyncio.create_task(runner.evaluate(config, suite, output, 500000))
        assert await asyncio.to_thread(mode["entered"].wait, 5)
        task.cancel()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task

    asyncio.run(run())
    report = json.loads((output / "report.json").read_text())
    assert report["status"] == "cancelled"
    assert len(mode["requests"]) == 1
    with sqlite3.connect(output / "smoke.db") as db:
        assert db.execute("SELECT status FROM tasks").fetchone()[0] == "cancelled"


def test_business_acceptance_is_type_strict(evaluation):
    from evaluation.reporting import accepts

    assert accepts('{"n":1}', {"n": 1})
    assert not accepts('{"n":true}', {"n": 1})
    assert not accepts('{"n":1.0}', {"n": 1})
    assert not accepts('{"n":1,"n":1}', {"n": 1})
    assert not accepts('{"n":1,"extra":0}', {"n": 1})


def test_overrun_stops_after_committed_cost(evaluation):
    runner, config, suite, output, mode = evaluation
    mode["tokens"] = 50000
    report = asyncio.run(runner.evaluate(config, suite, output, 500000))
    assert report["stop_reason"] == "budget"
    assert len(mode["requests"]) == 1
    assert report["groups"]["smoke"]["metrics"]["known_cost"] == 50005
    assert report["groups"]["smoke"]["no_artifact"] == 1


def test_rejected_artifacts_continue_and_remain_distinct_from_execution(evaluation):
    runner, config, suite, output, mode = evaluation
    mode["after_smoke"] = '{"name":"Wrong","count":2}'
    report = asyncio.run(runner.evaluate(config, suite, output, 500000))
    assert report["status"] == "completed"
    for group in ("single", "cascade"):
        assert report["groups"][group]["correct"] == 0
        assert report["groups"][group]["evaluated"] == 2
        assert report["groups"][group]["metrics"]["tasks"]["completed"] == 2


def test_invalid_json_artifacts_are_evaluated_and_continue(evaluation):
    runner, config, suite, output, mode = evaluation
    mode["after_smoke"] = "not JSON"
    report = asyncio.run(runner.evaluate(config, suite, output, 500000))
    assert report["status"] == "completed"
    assert all(r["accepted"] is False for r in report["runs"][1:])
    assert any(r["status"] == "human_required" for r in report["runs"][1:])


def test_transport_failure_stops_on_unknown_cost_without_fabricating_feedback(evaluation):
    runner, config, suite, output, mode = evaluation
    mode["http_error"] = True
    report = asyncio.run(runner.evaluate(config, suite, output, 500000))
    assert report["stop_reason"] == "unknown_cost"
    row = report["runs"][1]
    assert row["status"] == "failed"
    assert row["artifact"] is None
    assert row["accepted"] is None
    assert report["groups"]["single"]["metrics"]["feedback_revision"] == 0


def test_export_failure_is_reported_without_more_dispatch(evaluation, monkeypatch):
    runner, config, suite, output, mode = evaluation
    original = runner.Engine.metrics
    calls = 0

    async def fail_export(self):
        nonlocal calls
        calls += 1
        if calls > 5:
            raise RuntimeError("private-test-value")
        return await original(self)

    monkeypatch.setattr(runner.Engine, "metrics", fail_export)
    report = asyncio.run(runner.evaluate(config, suite, output, 500000))
    assert report["status"] == "incomplete"
    assert report["export_errors"]
    assert "private-test-value" not in (output / "report.json").read_text()
