"""Run single/cascade cohorts on a fixed synthetic extraction suite."""
import argparse
import asyncio
from pathlib import Path

from evaluation.runner import evaluate


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, type=Path)
    parser.add_argument("--suite", type=Path,
                        default=Path(__file__).with_name("evaluation-cases.json"))
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--total-budget", required=True, type=int, help="integer microcredits")
    args = parser.parse_args()
    try:
        report = asyncio.run(evaluate(args.config, args.suite, args.output, args.total_budget))
    except (KeyboardInterrupt, asyncio.CancelledError):
        parser.exit(130, "Interrupted; available evidence remains in the output directory.\n")
    except (ValueError, RuntimeError, OSError):
        parser.exit(2, "Evaluation could not start or export; check inputs, credentials and output.\n")
    print(f"{report['status']}: {args.output.resolve() / 'report.json'}")
    if report["status"] != "completed":
        parser.exit(1)


if __name__ == "__main__":
    main()
