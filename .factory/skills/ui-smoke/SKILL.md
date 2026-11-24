---
name: ui-smoke
description: Launch the Bromine window on a fixture file and report whether it actually started, since a passing test suite cannot show that a window renders. Use after any change under crates/bromine-ui or crates/bromine.
allowed-tools:
  - Execute
  - Read
---

# UI smoke

`cargo test` cannot tell you that a GPUI window renders. This skill collects the
evidence that is actually obtainable on this machine and is explicit about what
is not.

## Steps

1. Pick a fixture. Default to `fixtures/people.csv`; use `fixtures/quirky.csv`
   when the change touches quoting or ragged rows.
2. Build first, so a compile error is reported as a build failure rather than a
   mysterious launch failure: `cargo build -p bromine`
3. Launch it in the background, on a fixture file.
4. Wait a few seconds and check it is still alive. A GPUI app that panics on
   startup, or that fails to compile its shaders at runtime, exits almost
   immediately, so surviving is real evidence.
5. Confirm the process registered as a foreground GUI application:

   ```bash
   lsappinfo list | grep -A4 bromine
   ```

   A line containing `type="Foreground"` means macOS saw it as a real app rather
   than a background process.
6. Stop the process.
7. Report exactly what you observed.

## What this cannot prove

On this machine `screencapture` is refused (`could not create image from
display`) because the terminal running Droid does not hold Screen Recording
permission. So this skill cannot confirm the pixels are correct.

Do not claim the UI works. Report which of these you did:

- built cleanly, launched, survived, and registered as a foreground app
- anything less than that

Then ask the user to look at the window, or use the `droid-control` plugin
(`/verify`, `/qa-test`) for automated native-desktop evidence. That plugin needs
`cua-driver` and the Accessibility and Screen Recording permissions granted via
`cua-driver permissions grant`.

## Boundary

This skill never edits code. If the app fails to start, report the failure and
the relevant output; do not start fixing it unless the user asks.
