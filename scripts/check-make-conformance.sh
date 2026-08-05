#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# The gate measures the Make evaluator against GNU Make, so it runs the fork's
# own binary: what a Makefile evaluates to is decided before any front end
# builds it, and rkati is that evaluation with nothing else on top.
#
# Ronin's Make mode is the same evaluation followed by Ronin's scheduler, and
# the corpus can be pointed at it. Make mode is reached by the invoked name and
# by nothing else, so that means a make-named link rather than a flag:
#
#   ln -sf "$PWD/target/release/ronin" /tmp/ronin-make/make
#   scripts/check-make-conformance.sh --front-end /tmp/ronin-make/make
#
# That is not the gate, and the run says why: Ronin prints its own progress
# line where Make echoes each recipe, so nearly every case differs on that one
# line and on nothing else. Retargeting means reclassifying the corpus against
# a second contract, which belongs to whoever owns that decision rather than to
# whoever passes the flag.
cargo build --release -p kati --bin rkati
cargo build --release --bin ronin
cargo build --release --example make_conformance

exec target/release/examples/make_conformance "$@"
