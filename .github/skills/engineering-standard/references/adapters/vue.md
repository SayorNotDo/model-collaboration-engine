# Vue adapter

Use this adapter with the TypeScript adapter when Vue uses TypeScript. The Core Rules remain authoritative.

- **Boundaries and responsibilities (ENG-002/003/005):** Keep page orchestration, reusable UI, domain state, and API mapping distinct. Validate route params, form input, and API payloads before domain use.
- **State and effects (ENG-008/012):** Keep state as local as practical. Put reusable stateful behavior in composables and external effects behind explicit actions; avoid effects hidden in computed values.
- **Interfaces (ENG-009):** Keep props and emits narrow, typed, and domain-named. Do not expose a component's internal state shape merely for caller convenience.
- **Clarity (ENG-001/004/011):** Keep templates declarative; move nontrivial decisions into named computed values or functions. Split components by responsibility, not arbitrary line count.
- **Verification (ENG-013):** Test user-visible behavior and emitted contracts. Use component tests for interaction and end-to-end tests only for high-value journeys.
- **Tooling:** Prefer project scripts, commonly ESLint, Prettier, `vue-tsc --noEmit`, Vitest, Vue Test Utils, and Playwright or Cypress.
