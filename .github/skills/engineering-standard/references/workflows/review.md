# Review workflow

1. Establish the requested behavior, diff boundary, repository instructions, and profile strictness. Keep provisional profiles in memory; preserve tracked files during review.
2. Trace changed behavior through callers, boundaries, state transitions, effects, and error paths.
3. Evaluate every applicable Core Rule. Prioritize correctness, data loss, security, contract breaks, and missing risk-focused tests over style preferences.
4. Confirm findings with concrete code evidence and the conditions that trigger impact. Exclude speculative or purely subjective findings.
5. Inspect commands, hooks, and configuration before running checks. Select read-only/check variants; skip and explain checks requiring source changes or external mutation. Distinguish verified failures from unverified risk.
6. Report findings by severity with file and location, then note rule coverage and residual testing gaps. If no actionable findings exist, say so explicitly.
