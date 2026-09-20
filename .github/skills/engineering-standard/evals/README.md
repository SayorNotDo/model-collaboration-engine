# Usage-driven evaluations

This directory holds sanitized cases derived from actual use or review findings. It is a casebook and manual evaluation protocol, not an automated agent runner. Scanner unit tests remain in `../tests`. No end-to-end agent results are claimed by adding a case.

## Add a case

Use `cases/review-no-writes.yaml` as the field template. Give each case a stable ID, origin, minimal fixture, user prompt, and observable assertions. Keep the fixture synthetic and self-contained. Use explicit invocation to test instruction-following; omit the skill name in a separate case when testing automatic selection. Include negative examples for selection tests.

## Run a case

1. Materialize the fixture in a fresh temporary project, outside this skill and live repositories. Place evaluator artifacts outside the target project. Install or expose the selected skill revision using the host's normal skill discovery mechanism.
2. Snapshot project files, including untracked files. Run the recorded prompt in a fresh agent session with the case's permissions. Save the available execution trace, final response, and final filesystem diff. Do not include expected answers or assertion details in the agent prompt.
3. Evaluate every assertion using the specified evidence. Mark missing or ambiguous evidence as `unverified`, not passed. Check command attempts as well as effects: a blocked write attempt is still a behavior failure when prohibited by the case.
4. For comparison, restore the identical initial fixture and repeat with the old skill revision or without the skill. Keep model, reasoning settings, tools, permissions, and other installed skills fixed. For a no-skill baseline use the same task intent without explicit skill invocation, and make sure it is not available through another installation or conversation history.
5. Start with three runs per condition for important cases; report pass counts and variation rather than treating this small sample as a reliability guarantee. For subjective code quality, use a defined rubric and human spot checks; a model's self-report is not sufficient evidence.

## Result records

Keep raw traces local. If results need saving, use `evals/results/<case-id>/<run-id>.json`; this directory is ignored by Git. Share only a sanitized summary deliberately selected for publication. Each result should contain:

- `case_id`, `run_id`, `timestamp`, `condition` (current / previous / no-skill).
- `skill_revision`, `model`, `agent_version`, `settings`, `permissions`.
- `assertions`: list of `{id, status, evidence}`, status being pass / fail / unverified.
- `duration_seconds`, `tokens` (null if unavailable), `interventions`.
- `overall`: fail if any required assertion fails, unverified if any lacks evidence, otherwise pass.

For release, run the cases affected by the change and existing related regressions. Report behavior evaluation separately from script tests. A case file, code review, or static validation alone does not establish that an agent follows the instructions.
