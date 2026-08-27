#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

ninja_source=${NINJA_SOURCE:-"$repo_root/reference/ninja"}
ninja_build=${NINJA_BUILD:-"$repo_root/reference/ninja-build"}
ninja_binary=${NINJA_BINARY:-"$ninja_build/ninja"}
performance_warmups=${PERFORMANCE_WARMUPS:-2}
performance_repetitions=${PERFORMANCE_REPETITIONS:-15}
# Fewer than the Ninja gate's fifteen because these workloads are seconds
# rather than milliseconds: nine interleaved samples of each is a minute and a
# half, and fifteen would be four.
make_performance_warmups=${MAKE_PERFORMANCE_WARMUPS:-1}
make_performance_repetitions=${MAKE_PERFORMANCE_REPETITIONS:-9}

cargo fmt --all -- --check
cargo check --all-targets
# Every lint gate, in the one place the spelling lives. This used to be three
# hand-maintained lines here and a hand-typed split in each dispatch, which is
# how the two came to disagree; scripts/check-lints.sh is now what both run.
scripts/check-lints.sh
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo test --all-targets --no-fail-fast
scripts/check-make-equivalence.sh
# Two hand-written build systems, built from their own Makefiles. The corpus is
# a Makefile per feature and the generated-Makefile gate is one program's
# output; these are thirty years of maintenance using recursion the way a
# generator never would, and both of them found a defect nothing else had.
scripts/check-make-projects.sh

# The port's ledger, where `nplan port check --wave=4` used to be. That ladder
# gates a source annotation per symbol, and the C corpus those symbols came
# from was retired from this repository, so it read 0/170 and refused whatever
# the rest of the port did — which, with `set -eu`, is why nothing below this
# line ran for forty dispatches. The script asserts the three columns whose
# subject still exists and asserts the fourth is empty, so a source side coming
# back is itself a finding. See make-release-gate-stops-at-the-port-ladder.
scripts/check-port-ledger.sh

scripts/check-ninja-conformance.sh \
    --ninja-source "$ninja_source" \
    --ninja-build "$ninja_build"

scripts/check-performance.sh \
    --ninja "$ninja_binary" \
    --ninja-source "$ninja_source" \
    --warmups "$performance_warmups" \
    --repetitions "$performance_repetitions"

# The same question for the other front end, against the tool it stands in for.
# It runs after check-make-projects.sh above, and has to: its two real
# workloads measure vim and zsh at their up-to-date steady state, and there is
# nothing up to date until that gate has built one. The clean build of vim is
# recorded rather than gated — sixteen seconds a side is too much to spend on
# every release pass, and `--clean-build` is how you ask for it.
scripts/check-make-performance.sh \
    --warmups "$make_performance_warmups" \
    --repetitions "$make_performance_repetitions"

uncovered=$(nplan spec uncovered --prefix samurai --color never)
printf '%s\n' "$uncovered"
if ! printf '%s\n' "$uncovered" |
    rg -q '^(No uncovered rules\.|0 uncovered rule\(s\):)'; then
    echo "release gate: uncovered specification rules remain" >&2
    exit 1
fi

stale=$(nplan spec stale --prefix samurai --color never)
printf '%s\n' "$stale"
stale_rules=$(printf '%s\n' "$stale" | sed -n 's/^  \([^:]*\):.*/\1/p' | sort -u)
for rule in $stale_rules; do
    if rg -q "\[spec:ronin:req:${rule}(\+[0-9]+)?\]" docs/spec; then
        echo "release gate: stale requirement annotation for $rule" >&2
        exit 1
    fi
done

nplan lint
nplan audit

# The gate used to build a registry package here and check that planning-only
# and legacy files stayed out of it. Ronin is `publish = false`: it is a binary
# tool, and it carries the Make frontend as a path dependency on a submodule,
# which no registry crate can express. Building one therefore fails for a
# reason that says nothing about the release.
#
# The leak check went with it because it had no subject once the artifact it
# inspected stopped being built. If a distribution artifact is ever added, that
# check belongs to it, not here — and it should inspect the thing actually
# shipped rather than a crate nobody consumes.
