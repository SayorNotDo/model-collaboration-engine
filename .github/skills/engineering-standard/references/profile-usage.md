# Profile interpretation

Version 0.2.0 stores each quality category as an array. Each entry names a tool, a nullable command, its stack scope, and `verification: inspect`. Empty arrays mean no tool was detected, not that the project needs no checks. A null command means tooling was detected but its invocation needs repository evidence. The scanner executes no project commands and does not certify their safety.

Inspect scripts, hooks, tool configuration, and test/build effects before choosing commands. Prefer existing project commands and installed executables; do not install packages just to resolve a null command. Respect the package manager, environment, workspace, and target package. In review, select check-only commands and report any unavailable verification. Test commands can also mutate snapshots or external systems.

For existing 0.1.0 profiles, interpret a non-null `{name, command}` object as a single entry requiring inspection; an all-null object becomes an empty list in memory. Preserve the file during review. When explicitly migrating during implementation, wrap each existing tool object in a list, add `scope: unknown` and `verification: inspect`, and set version to 0.2.0. Preserve project choices and commands. Report unsupported versions instead of guessing their meaning.

The MVP scans manifests/configuration at the specified root only. Scan workspace packages separately when relevant; it does not merge a monorepo automatically. Architecture remains `unknown` until supported by repository evidence. `agent.target` is metadata, not an installer or a runtime integration for other agents. Apply listed `agent.instructions` as repository-relative instruction-file paths when they exist, subject to higher-priority instructions.
