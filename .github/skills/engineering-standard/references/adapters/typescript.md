# TypeScript adapter

This adapter maps Core Rules to TypeScript practice; the Core Rules remain authoritative.

- **Boundaries and invariants (ENG-003/005/006):** Treat decoded JSON and browser input as `unknown`; narrow or parse at the boundary. Use discriminated unions for meaningful state variants.
- **Interfaces and effects (ENG-009/012):** Prefer narrow structural types and dependency parameters at real seams. Keep fetch, storage, time, and environment access outside pure domain decisions.
- **Errors (ENG-007):** Preserve causes and distinguish expected domain outcomes from exceptional failures. Avoid unchecked `as` assertions and swallowed promise rejections.
- **Clarity (ENG-001/002/004/011):** Keep strict compiler settings when the project supports them. Prefer exhaustive switching over boolean combinations for state machines.
- **Verification (ENG-013):** Use the configured runner, commonly Vitest, Jest, or Node's test runner. Assert observable outputs and failure behavior rather than private calls.
- **Tooling:** Prefer repository scripts. Typical tools are Prettier, ESLint or Biome, and `tsc --noEmit`.
