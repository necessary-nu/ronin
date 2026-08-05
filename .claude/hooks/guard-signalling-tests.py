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

# Two hazards, one rule. A suite either signals its parent or spawns without a
# bound of its own, and both end the same way when run in the session's tree.
#
# The list is deliberately about corpora rather than about the specific tests
# that have misbehaved. `run_make_tests` was not here on the day it took the
# machine down, because the list then named only what had already gone wrong.
PARENT_SIGNALLING = (
    (r"check-ninja-conformance", "runs upstream ninja_test, which signals its parent"),
    (r"\bninja_test\b", "is upstream ninja_test, which signals its parent"),
    (r"check-release", "runs the conformance gate, and so ninja_test"),
    (r"\bcargo\s+(test|nextest)\b", "runs subprocess::tests and signal::tests"),
    (r"run_make_tests", "is GNU Make's suite, which has multiplied without bound"),
    (r"check-make-conformance", "runs the Make corpus, hundreds of spawns"),
    (r"check-make-equivalence", "evaluates the Make corpus"),
    (r"\b(sinkcmp|matrix)\.py\b", "runs the emitter corpus, hundreds of spawns"),
)

# What counts as already isolated: the repo's wrapper, or a hand-rolled
# invocation that unshares the pid namespace itself.
ISOLATED = (
    re.compile(r"scripts/sandboxed\b"),
    re.compile(r"\bsandbox\b[^|;&]*--unshare[= ][^|;&]*\bpid\b"),
)

# Naming a suite is not running one. `ls .../run_make_tests.pl` and
# `grep -n kNumProcs subprocess_test.cc` are how you find out what a thing does
# before deciding whether to run it, and a guard that refuses them is one that
# gets switched off. Only a command whose *first word* reads rather than
# executes is exempt — a pipeline or a `&&` returns to the normal rule, since
# anything after the first stage can execute.
INSPECTORS = frozenset(
    "ls cat head tail grep rg sed awk wc stat file find test realpath basename"
    " dirname readlink echo printf diff cmp md5sum sha256sum git".split()
)


def is_inspection(command: str) -> bool:
    """True when the command only looks at things, so a mention is not a run."""
    if re.search(r"[|;&]|\$\(|`", command):
        return False
    first = command.strip().split()
    return bool(first) and first[0].rsplit("/", 1)[-1] in INSPECTORS

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

    if is_inspection(command):
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
