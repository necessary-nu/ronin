#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

ninja_source=${NINJA_SOURCE:-/tmp/ninja}
ninja_build=${NINJA_BUILD:-/tmp/ninja-build}
ninja_binary=${NINJA_BINARY:-"$ninja_build/ninja"}
performance_warmups=${PERFORMANCE_WARMUPS:-2}
performance_repetitions=${PERFORMANCE_REPETITIONS:-15}

cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings -W clippy::pedantic -W clippy::nursery
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo test --all-targets --no-fail-fast

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

package_files=$(cargo package --list)
printf '%s\n' "$package_files"
if printf '%s\n' "$package_files" |
    rg -q '(^|/)(plan/|\.config/|[^/]+\.(c|h)$|Makefile$|samu\.1$)'; then
    echo "release gate: package contains legacy or planning-only files" >&2
    exit 1
fi
cargo package
