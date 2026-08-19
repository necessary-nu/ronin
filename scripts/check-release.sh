#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

ninja_source=${NINJA_SOURCE:-"$repo_root/reference/ninja"}
ninja_build=${NINJA_BUILD:-"$repo_root/reference/ninja-build"}
ninja_binary=${NINJA_BINARY:-"$ninja_build/ninja"}
performance_warmups=${PERFORMANCE_WARMUPS:-2}
performance_repetitions=${PERFORMANCE_REPETITIONS:-15}

cargo fmt --all -- --check
cargo check --all-targets
# Clippy in three passes, because one command cannot say three different things
# about three different bodies of code. The single line this replaced said all
# of them at once and therefore reached none of them: `-W clippy::pedantic`
# applies to every crate cargo builds, kati is a path dependency, and the run
# died on the fork's ~600 inherited findings before it ever looked at Ronin. So
# the gate has been red for weeks and every dispatch has run the split by hand.
#
# Ronin's own product code, at the house standard. `--no-deps` is what keeps the
# groups off the fork. The groups are also declared in Cargo.toml's `[lints]`,
# so this is belt and braces rather than the only statement of the standard.
cargo clippy --lib --bins --no-deps -- -D warnings -W clippy::pedantic -W clippy::nursery
# Ronin's tests, examples and benches, at the ordinary standard. The pedantic
# and nursery groups come off here and only here: the harnesses carry a handful
# of findings the product code does not — `assigning_clones` where a parser
# writes a field three times, `option_if_let_else` where a match reads better
# than the closure pair it suggests — and rewriting them would not make a test
# say anything truer. Everything that is about correctness still applies.
cargo clippy --all-targets --no-deps -- -D warnings -A clippy::pedantic -A clippy::nursery
# The Make front end, separately and without the pedantic groups: it is a
# vendored fork carrying ~600 pedantic and nursery findings it inherited, and
# rewriting Google's code to Ronin's house style is not this gate's business.
# Plain `-D warnings` is, and it is what the fork's own `#![deny(warnings)]`
# used to do — moved here so that a rustc release that adds a lint fails a
# check someone is running rather than every build of the tree.
cargo clippy -p kati --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo test --all-targets --no-fail-fast
scripts/check-make-equivalence.sh
# Two hand-written build systems, built from their own Makefiles. The corpus is
# a Makefile per feature and the generated-Makefile gate is one program's
# output; these are thirty years of maintenance using recursion the way a
# generator never would, and both of them found a defect nothing else had.
#
# Ahead of the port ladder deliberately: that check is red for a bookkeeping
# reason (make-release-gate-stops-at-the-port-ladder) and `set -eu` means
# nothing below it runs, so a gate placed after it would not be a gate.
scripts/check-make-projects.sh

nplan port check --wave 4

scripts/check-ninja-conformance.sh \
    --ninja-source "$ninja_source" \
    --ninja-build "$ninja_build"

scripts/check-performance.sh \
    --ninja "$ninja_binary" \
    --ninja-source "$ninja_source" \
    --warmups "$performance_warmups" \
    --repetitions "$performance_repetitions"

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
