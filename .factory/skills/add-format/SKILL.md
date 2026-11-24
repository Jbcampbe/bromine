---
name: add-format
description: Add support for another developer data format (JSON, Markdown, YAML, TOML) to Bromine. Use when the user asks to open, view, or edit a new file format.
---

# Add a format

Bromine follows one shape for every format. Adding a format means walking these
steps in order, core first, because the core steps are the ones that can be
verified.

## 1. Core engine in `crates/bromine-core`

- Add a module beside `csv.rs` and export it from `lib.rs`.
- Model the format as data, not as widgets. It is fine for a format to have its
  own model when a `Table` does not fit (a JSON tree and a Markdown document
  both will). What is not fine is a model that only exists inside a view.
- Reuse `CoreError`. Add a variant rather than returning a string.
- Round-tripping is a requirement for anything editable: if the app can write a
  format, prove that `parse -> write -> parse` is a fixed point.

## 2. A fixture in `fixtures/`

- Add a small file that exercises the format's awkward cases: nesting, escapes,
  empty containers, non-UTF-8 bytes if the format allows them.
- Document it in `fixtures/README.md`.
- Fixtures are covered by a `PreToolUse` hook: add a new one freely, but do not
  modify an existing one to make a test pass.

## 3. Contract tests

- Add a test module in the engine, plus a case in
  `crates/bromine-core/tests/fixtures.rs` if the fixture is committed.
- Test the awkward cases from step 2 explicitly, not just the happy path.

## 4. Register the format

- Extend `bromine_core::Format` and its `from_path` so the binary can dispatch
  on the file extension.

## 5. Only then, the view

- Add a view under `crates/bromine-ui`. Reuse the shared theme tokens rather
  than inline colors.
- Keep state on the view entity and mutate it in event handlers, then call
  `cx.notify()`.
- Add a headless `#[gpui::test]` for the interaction logic.

## 6. Verify

Run the `check` skill, then the `ui-smoke` skill on the new fixture. A format is
not done until a human has looked at it.
