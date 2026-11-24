---
name: check
description: Run Bromine's full verification gate (fmt, clippy, tests) and report exactly what failed. Use when finishing a change, before committing, or when the user asks whether the project is green.
---

# Check

Run the repository's verification commands in this order and report the real
result of each. Do not summarize a failure as "minor" and do not stop after the
first command unless it fails.

1. `cargo fmt --all` (then `cargo fmt --all --check` to confirm it is clean)
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`

## Reporting

- For each command, state pass or fail and give the command, not a paraphrase.
- On a test failure, quote the failing test name and its assertion output.
- On a clippy failure, quote the lint name and the file and line.
- If everything passes, report the test counts as they appear (for example
  "41 passed; 0 failed"), because a suite that suddenly runs fewer tests is
  itself worth noticing.
- If a command could not run, say which and why. Never report a command you did
  not run.

## Boundaries

- Fix failures you introduced. If a failure predates your change, say so and
  leave it, rather than folding an unrelated fix into the work.
- Never edit `fixtures/` to make a test pass. Fixtures are the contract between
  the core tests, the views, and QA runs. See `AGENTS.md`.
