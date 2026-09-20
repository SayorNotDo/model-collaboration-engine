# Rust adapter

This adapter maps Core Rules to Rust practice; the Core Rules remain authoritative.

- **Boundaries and invariants (ENG-003/005/006):** Parse into domain newtypes or enums. Use constructors when field combinations have invariants; make invalid states difficult to express.
- **Interfaces and effects (ENG-009/012):** Keep traits narrow and introduce them at genuine substitution boundaries. Make ownership explicit; prefer borrowing unless transfer communicates intent.
- **Errors (ENG-007):** Return `Result` for recoverable failure, preserve sources, and reserve panics for violated internal invariants. Avoid `unwrap` on external or runtime-dependent paths.
- **Clarity (ENG-001/002/004/011):** Use exhaustive matching and cohesive modules. Prefer explicit transformations over clever lifetime or generic machinery without demonstrated need.
- **Verification (ENG-013):** Combine focused unit tests with integration tests at public boundaries. Test error variants and edge values.
- **Tooling:** Run the profile commands, commonly `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`.
