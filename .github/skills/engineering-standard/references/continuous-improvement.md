# Improve from actual use

## Capture evidence

Complete the user's current task first, unless the failure blocks it. When the skill causes an observed problem, record four things: scenario, expected behavior, actual behavior, and evidence. Use `assets/usage-feedback.md` for a reusable record when needed; a short handoff note is enough for a single finding. Identify the skill Git revision when available; the profile schema version alone does not identify the instructions used. Record unknown values as unknown.

Keep review feedback in the response. Write a project feedback file only when requested or covered by the project's existing workflow. Ordinary skill use does not authorize changes to the installed skill, commits, or publishing. A user request to improve the skill does authorize local implementation and verification; follow the existing session's publishing instructions when applicable.

Retain only evidence necessary to reproduce the behavior. Prefer synthetic fixtures over copying private project files. Remove credentials, personal data, private URLs, and proprietary payloads before adding anything to the shared skills repository. Link to private evidence only in the originating project's approved location.

## Triage before changing rules

- Project convention or stale profile: correct project configuration within the task's scope.
- Scanner defect: reproduce with a deterministic scanner test.
- Unclear routing or instruction: create an agent behavior case before editing the relevant instruction.
- Stack-specific gap: update the adapter when the evidence generalizes to that stack.
- General engineering gap: change Core Rules only if existing rules do not already cover it.
- Uncertain or isolated model behavior: retain the observation and repeat the case before inferring a broad instruction defect. One reproducible serious failure is enough to justify a narrow fix.

The goal is an observable improvement, not more rules. Remove redundant instructions when a narrower wording resolves the issue.

## Reproduce, compare, release

When asked to improve this skill, follow `../evals/README.md`: turn the finding into a sanitized case with checkable success criteria, then make the smallest supported change. Run the relevant script regression or agent case and check affected existing cases. Keep held-out scenarios when tuning instructions so the examples used to write the fix are not the only evidence.

Report case IDs, skill revisions, what ran, what passed or failed, and what remains untested. Script tests and document validation do not count as agent behavior evaluations. Publish only within the user's existing authorization; summarize changes and known limitations so other computers can update deliberately.
