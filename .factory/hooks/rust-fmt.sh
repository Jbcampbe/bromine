#!/usr/bin/env bash
# Formats Rust code after a file write.
#
# Deliberately cheap. This runs on every Create/Edit/ApplyPatch, so it must not
# do anything slower than `cargo fmt`. Clippy and tests belong in the `check`
# skill, not here.
#
# Hook input arrives as JSON on stdin and is not needed for formatting.
set -uo pipefail

project="${FACTORY_PROJECT_DIR:-$(pwd)}"
cd "$project" 2>/dev/null || exit 0

# Never let a missing toolchain turn into a failed hook.
command -v cargo >/dev/null 2>&1 || exit 0

# Formatting is a convenience, not a gate: failures are reported to the user but
# must not block the edit that triggered them.
if ! output="$(cargo fmt --all 2>&1)"; then
  printf 'cargo fmt failed:\n%s\n' "$output" >&2
fi

exit 0
