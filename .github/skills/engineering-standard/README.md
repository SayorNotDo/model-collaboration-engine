# Engineering Standard Skill

A portable MVP for applying consistent engineering-quality constraints across repositories. Copy this directory into a Codex skills directory or keep it inside a repository and invoke its `SKILL.md`.

## Responsibility boundary

Superpowers-style workflows answer **how the agent executes a task**: discovery, brainstorming, planning, decomposition, debugging, and iterative verification.

Engineering Standard answers **what quality the resulting change must satisfy**: boundaries, responsibilities, complexity, naming, validation, errors, tests, dependencies, and change scope.

They compose naturally: use the execution workflow to organize the work and this skill to constrain the implementation and review it. This MVP does not include a full CLI, database, or Web UI.

## Layout

```text
engineering-standard/
├── SKILL.md
├── agents/openai.yaml
├── assets/project-profile.yaml
├── references/
│   ├── core-rules.yaml
│   ├── project-profile.schema.yaml
│   ├── adapters/{python,typescript,rust,vue}.md
│   └── workflows/{feature,bugfix,refactor,review}.md
└── scripts/scan_project.py
```

## Quick start

```bash
python scripts/scan_project.py /path/to/repository
python scripts/scan_project.py /path/to/repository --output /path/to/repository/.engineering/project-profile.yaml
```

Run these examples from the skill directory, or use the quoted absolute path to its scanner from another directory. The scanner requires Python 3.11+ and uses only its standard library. It preserves an existing output file unless `--force` is supplied. Invalid or unreadable manifests fail with an error instead of producing an empty profile.

The scanner detects root-level manifests, common JavaScript dependency/configuration evidence, package scripts, Python tool configuration and dependency groups, and Rust tools. It does not execute project code or recursively scan installed dependencies. Monorepo packages require separate scans. Tools detected without a known invocation have a null command; an agent must resolve it from repository evidence. All commands require inspection, particularly during read-only review.

## Profile lifecycle

The default scan writes to stdout only. Implementation work can persist `.engineering/project-profile.yaml` using `--output`; reviews use provisional profiles in memory. Later runs read the profile. Edit it when project tooling or architecture changes. Version 0.2.0 uses tool arrays so multiple stacks retain their checks. See `references/profile-usage.md` for 0.1.0 compatibility and migration.

The Core Rules are original summaries inspired by Steve McConnell's *Code Complete* (complexity, construction, defensive programming, testing) and Robert C. Martin's *Clean Code* (naming, responsibilities, interfaces, errors). They are not quotations or a complete implementation of either book.

## Improve through actual use

When a task exposes a skill-related problem, record scenario, expected behavior, actual behavior, and evidence using `assets/usage-feedback.md`. The agent routes observed failures through `references/continuous-improvement.md`. Project-specific issues belong in project configuration; reusable defects become scanner tests or behavior cases.

See `evals/README.md` for the manual comparison protocol and `evals/cases/review-no-writes.yaml` for a sanitized case derived from the earlier review. The case has not been run as an agent evaluation. Normal use records feedback without automatically changing or publishing the skill. Raw evaluation results stay local and are ignored by Git.

## Verification

From this directory:

```bash
python -m unittest discover -s tests -v
python scripts/scan_project.py .
python /path/to/skill-creator/scripts/quick_validate.py .
```

The scanner tests use the standard library. Optional structural validation uses the development-only packages PyYAML and jsonschema: install them in a development environment, then run `python tests/validate_artifacts.py`. These are not scanner runtime dependencies.
