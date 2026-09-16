"""Import a synthetic or third-party local ranking without modifying engine configuration."""
import argparse
import json
from pathlib import Path

from model_collaboration_engine import load_rankings


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", dest="source", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    result = load_rankings(args.source, args.manifest)
    outputs = [(args.output, result["rankings"])]
    if args.report is not None:
        outputs.append((args.report, result["report"]))
    paths = [path.resolve() for path, _ in outputs]
    if len(set(paths)) != len(paths) or any(path.exists() for path in paths):
        parser.error("output and report must be distinct new files; overwrite is refused")
    for path, value in outputs:
        with path.open("x", encoding="utf-8") as handle:
            json.dump(value, handle, ensure_ascii=False, indent=2, allow_nan=False)
            handle.write("\n")
    print(json.dumps(result["report"], ensure_ascii=False))


if __name__ == "__main__":
    main()
