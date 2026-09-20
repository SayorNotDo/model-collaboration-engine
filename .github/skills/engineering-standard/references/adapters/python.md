# Python adapter

This adapter maps Core Rules to Python practice; the Core Rules remain authoritative.

- **Boundaries and invariants (ENG-003/005/006):** Parse external data at entry points. Use dataclasses, enums, typed constructors, or existing validation libraries to represent valid domain values; avoid adding a validation dependency for simple checks.
- **Interfaces and effects (ENG-009/012):** Prefer small protocols or callable parameters at real substitution seams. Pass clocks, clients, and storage collaborators when deterministic tests need control.
- **Errors (ENG-007):** Raise specific exceptions with causal chaining (`raise ... from error`). Catch only where recovery, translation, or context is added.
- **Clarity (ENG-001/002/004/011):** Follow repository typing and naming conventions. Keep orchestration readable and move low-level parsing or I/O into cohesive helpers.
- **Verification (ENG-013):** Use the configured runner, commonly pytest or unittest. Cover public behavior, boundary values, and expected exception paths; use temporary directories and fakes for effects.
- **Tooling:** Prefer the profile commands. Common evidence includes Ruff or Flake8 for lint, Black or Ruff for format, and mypy or Pyright for type checks.
