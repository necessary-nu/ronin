#!/usr/bin/env python3
"""Refuse to run this repo's parent-signalling test suites in the session's tree.

Upstream Ninja's `SubprocessTest.InterruptParentWithSigTerm` exists to have a
child signal its parent: the recipe it runs is literally `kill -TERM $PPID`.
That is correct when the parent is `ninja_test`. Run from an agent's process
tree, the same signal has a path toward things that are not `ninja_test` — and
on 2026-08-05 it reached the session and killed it.

Ronin's own `subprocess::tests` and `signal::tests` are tamer: they kill process
groups they created, and assert that signalling PID 2147483647 fails. They are
covered here anyway, because the distinction is not one worth relearning at the
moment a suite is invoked.

The rule is not "avoid the dangerous test" but "these suites do not run in the
session's process tree". `scripts/sandboxed` gives them their own, so a stray
signal has nowhere to travel.

Exit 0 to allow. Print a JSON deny and exit 0 to refuse — the hook contract
reads the decision from stdout, not from the status.
"""

import json
import re
import sys

# Anything that runs upstream's ninja_test, directly or through a script, plus
# our own suites that spawn and signal process groups.
PARENT_SIGNALLING = (
    (r"check-ninja-conformance", "runs upstream ninja_test"),
    (r"\bninja_test\b", "is upstream ninja_test"),
    (r"check-release", "runs the conformance gate, and so ninja_test"),
    (r"\bcargo\s+(test|nextest)\b", "runs subprocess::tests and signal::tests"),
)

# What counts as already isolated: the repo's wrapper, or a hand-rolled
# invocation that unshares the pid namespace itself.
ISOLATED = (
    re.compile(r"scripts/sandboxed\b"),
    re.compile(r"\bsandbox\b[^|;&]*--unshare[= ][^|;&]*\bpid\b"),
)

ADVICE = (
    "Refused: this command {because}, and it is not isolated from the session.\n\n"
    "Upstream Ninja's SubprocessTest.InterruptParentWithSigTerm runs "
    "`kill -TERM $PPID` by design. In the session's own process tree that signal "
    "has a path out of the test and into the session — it has killed one before.\n\n"
    "Run it through the wrapper, which puts it in its own pid namespace:\n"
    "    scripts/sandboxed {command}\n\n"
    "If it genuinely cannot be isolated, say so and ask rather than working around "
    "this."
)


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        return 0

    command = (payload.get("tool_input") or {}).get("command") or ""
    if not command:
        return 0

    if any(pattern.search(command) for pattern in ISOLATED):
        return 0

    for pattern, because in PARENT_SIGNALLING:
        if re.search(pattern, command):
            print(
                json.dumps(
                    {
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": ADVICE.format(
                                because=because, command=command.strip()
                            ),
                        }
                    }
                )
            )
            return 0

    return 0


if __name__ == "__main__":
    sys.exit(main())
