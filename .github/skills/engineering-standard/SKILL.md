---
name: engineering-standard
description: Apply portable engineering-quality constraints to feature, bugfix, refactor, and code-review work. Use when implementing or reviewing code that should follow project-aware architecture, readability, validation, testing, and change-scope standards; complements task-execution workflows rather than replacing them.
---

# Engineering Standard

Apply the repository's established conventions first, then use this skill to fill gaps and make quality checks explicit.

## Route the task

1. Determine the task mode first. Resolve `<skill-dir>` from this SKILL.md location and `<repo>` from the target repository. All references below are relative to `<skill-dir>`; execute project checks with `<repo>` as working directory. Use Python 3.11+.
2. Locate `<repo>/.engineering/project-profile.yaml`. If absent, run `python "<skill-dir>/scripts/scan_project.py" "<repo>"` and use its stdout as a provisional profile. In review mode keep it in memory. For an implementation task, persist it only as part of the scoped project setup. Preserve existing profiles. Read `references/project-profile.schema.yaml` when interpreting, creating, or updating a profile; read `references/profile-usage.md` for command selection and version compatibility.
3. Read `references/core-rules.yaml`. Treat `required` rules as gates; apply `recommended` rules unless repository constraints justify an exception.
4. Read only the adapters named by `project.languages` and `project.frameworks` in the profile: `references/adapters/python.md`, `typescript.md`, `rust.md`, or `vue.md`.
5. Read exactly one matching workflow: `references/workflows/feature.md`, `bugfix.md`, `refactor.md`, or `review.md`.

## Resolve strictness

- `strict`: every applicable required and recommended rule is a gate. Record any exception with evidence.
- `balanced`: required rules are gates; recommended rules guide tradeoffs.
- `pragmatic`: required rules remain gates when applicable; prefer the smallest safe change and record deferred improvements.

Repository instructions and user requirements outrank this skill. Resolve a conflict explicitly in the delivery summary.

## Complete the task

Before delivery, account for every applicable Core Rule using implementation evidence, automated checks, or a concise exception. Inspect every relevant command and its configuration before execution, including package pre/post hooks and delegated scripts. Review mode permits only checks that do not modify tracked source, configuration, snapshots, or external systems; use check-only variants or report that verification was not run. Never run format/write/fix/update commands merely to review. Normal local disposable test/build output is acceptable after checking effects. A null command is unresolved, not a passed or unnecessary check. Report behavior and boundaries changed, verification outcomes, exceptions or residual risks, and unrelated opportunities left untouched.

Completion is observable: requested behavior is present, applicable configured checks pass, and every applicable required rule has evidence or an explicit exception.

When actual use exposes a missed constraint, incorrect tool choice, or unnecessary work caused by this skill, read `references/continuous-improvement.md` and include a concise evidence-backed feedback note in the handoff. Read it also when the user requests improvement from usage feedback. Ordinary successful tasks need no feedback artifact. Recording a finding does not initiate a skill edit or release.
