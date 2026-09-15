"""Run from the repository root after configuring config/engine.json."""
import asyncio
import json
import time
from pathlib import Path
from uuid import uuid4

from model_collaboration_engine import Engine, parse_config


async def main() -> None:
    root = Path(__file__).resolve().parent
    config_dir = root.parent / "config"
    document = json.loads((config_dir / "engine.json").read_text(encoding="utf-8"))
    # Host explicitly selects planning candidates; data constraints still apply.
    document["routing"]["planner_models"] = [document["providers"][0]["models"][0]["id"]]
    config = parse_config(document, base_dir=config_dir)
    submission = json.loads((root / "submission.json").read_text(encoding="utf-8"))
    submission["task_id"] = str(uuid4())
    submission["deadline_ms"] = int(time.time() * 1000) + 60_000
    async with await Engine.open(config) as engine:
        async with engine.stream(submission) as run:
            async for event in run:
                if event["kind"] == "content_delta":
                    print(event["data"]["text"], end="", flush=True)
                else:
                    print(f"\n[{event['kind']}] {event['data']}")
            result = await run.result()
            print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    asyncio.run(main())
