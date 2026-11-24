#!/usr/bin/env python3
"""Blocks changes to committed test fixtures.

Fixtures are the contract between the core tests, the GPUI views, and QA runs.
An agent that cannot make a test pass by fixing code must not be able to make it
pass by editing the sample data instead.

Allowed:
  * Creating a fixture file that does not exist yet.
  * Anything outside `fixtures/`.
  * Anything, when BROMINE_ALLOW_FIXTURE_EDIT=1 is set in Droid's environment.

Blocked:
  * Editing, patching, or overwriting a fixture that already exists.

Blocking works by exiting 2 with the reason on stderr.
"""

import json
import os
import sys

EXIT_ALLOW = 0
EXIT_BLOCK = 2

FIXTURE_DIR = "fixtures/"
OVERRIDE_ENV = "BROMINE_ALLOW_FIXTURE_EDIT"

# Prefixes that unified diff headers put in front of a real path.
DIFF_PREFIXES = ("--- ", "+++ ", "--- a/", "+++ b/", "a/", "b/", "--- ", "+++ ")


def read_payload() -> dict:
    try:
        return json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        # Nothing usable on stdin: stay out of the way rather than blocking.
        print("protect-fixtures: could not read hook input as JSON", file=sys.stderr)
        return {}


def strip_diff_prefix(token: str) -> str:
    for prefix in DIFF_PREFIXES:
        if token.startswith(prefix):
            return token[len(prefix) :]
    return token


def candidate_paths(payload: dict) -> list[str]:
    """Every path this tool call could touch.

    `Edit` and `Create` name a `file_path`. `ApplyPatch` carries no path field at
    all, so its string values are scanned for fixture paths in both plain and
    unified-diff form.
    """
    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return []

    path = tool_input.get("file_path")
    if isinstance(path, str) and path:
        return [path]

    found = []
    for value in tool_input.values():
        if isinstance(value, str):
            for line in value.splitlines():
                for token in line.split():
                    token = strip_diff_prefix(token.strip())
                    if token.startswith(FIXTURE_DIR):
                        found.append(token)
        elif isinstance(value, list):
            for item in value:
                if isinstance(item, str) and item.startswith(FIXTURE_DIR):
                    found.append(item)
    return found


def relative_fixture(path: str, project: str) -> str | None:
    """Return the repo-relative path when `path` is an existing fixture."""
    resolved = path if os.path.isabs(path) else os.path.join(project, path)
    try:
        resolved = os.path.realpath(resolved)
        relative = os.path.relpath(resolved, os.path.realpath(project))
    except (OSError, ValueError):
        return None

    if relative.startswith(".."):
        return None
    if not relative.startswith(FIXTURE_DIR):
        return None
    if not os.path.exists(resolved):
        return None
    return relative


def main() -> int:
    payload = read_payload()
    project = os.environ.get("FACTORY_PROJECT_DIR") or payload.get("cwd") or os.getcwd()

    touched = [relative_fixture(path, project) for path in candidate_paths(payload)]
    touched = sorted({path for path in touched if path})

    if not touched:
        return EXIT_ALLOW

    if os.environ.get(OVERRIDE_ENV) == "1":
        json.dump(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason": f"{OVERRIDE_ENV}=1 is set",
                }
            },
            sys.stdout,
        )
        return EXIT_ALLOW

    print(
        "Refusing to modify an existing test fixture: {paths}.\n"
        "Fixtures are shared by the core tests, the views, and QA runs, so a "
        "failure here is a statement about the code, not about the sample data.\n"
        "Fix the code, add a new fixture file, or -- if the fixture genuinely is "
        "wrong -- restart Droid with {env}=1 set in the environment.".format(
            paths=", ".join(touched), env=OVERRIDE_ENV
        ),
        file=sys.stderr,
    )
    return EXIT_BLOCK


if __name__ == "__main__":
    sys.exit(main())
