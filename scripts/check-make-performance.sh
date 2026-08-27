#!/bin/sh
# Wall-time gate for Ronin's MAKE mode against GNU Make 4.4.1.
#
# scripts/check-performance.sh is the same gate for NINJA mode against pinned
# stock Ninja, and it was the only performance gate this repository had. Make
# mode had never been compared with the tool it stands in for, so nothing could
# say whether using Ronin as `make` is faster or slower than GNU Make — and
# nothing would have said so if it got worse.
#
# The oracle is the same GNU Make the conformance and equivalence gates use:
# reference/make-oracle/make-4.4.1/make, built by scripts/build-make-oracle.sh.
# The trees are the ones scripts/check-make-projects.sh fetches, configures and
# builds, which is why that gate runs before this one in check-release.sh: the
# no-op workloads measure an up-to-date tree, and there is nothing up to date
# until it has built one.
#
# Make mode is reached by the invoked name and by nothing else, so the Ronin
# side runs through a `make`-named symlink rather than through target/release.
#
# Usage: scripts/check-make-performance.sh [--clean-build] [extra baseline args]
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

gnu_make=${MAKE_ORACLE:-"$repo_root/reference/make-oracle/make-4.4.1/make"}
if [ ! -x "$gnu_make" ]; then
    echo "check-make-performance: no GNU Make oracle at $gnu_make." >&2
    echo "Run scripts/build-make-oracle.sh first." >&2
    exit 1
fi

cargo build --release --bin ronin --example make_baseline

bin=$repo_root/target/make-performance-bin
rm -rf "$bin"
mkdir -p "$bin"
ln -s "$repo_root/target/release/ronin" "$bin/make"

exec target/release/examples/make_baseline \
    --gnu-make "$gnu_make" \
    --ronin-make "$bin/make" \
    --validate "$@"
