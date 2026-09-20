# Feature workflow

1. Define the observable behavior, non-goals, trust boundaries, and compatibility constraints. Complete when each acceptance case maps to a system boundary or domain responsibility.
2. Trace the smallest implementation path through existing architecture. Complete when every changed module has a stated responsibility and unrelated cleanup is excluded.
3. Design inputs, outputs, invariant enforcement, failure semantics, and effect boundaries before editing. Complete when invalid and failure cases have explicit outcomes.
4. Implement a thin end-to-end behavior slice, then fill supporting cases without speculative abstractions.
5. Add risk-proportional tests for success, boundary, and failure paths. Run applicable profile commands.
6. Review every applicable Core Rule and deliver evidence, exceptions, and residual risks.
