#!/bin/sh
set -eu

# Every Makefile in the corpus must build the same graph two ways: directly,
# and by parsing the Ninja manifest the same evaluation emits. That is
# [spec:ronin:req:make.manifest-equivalence], and it is what lets the emitter
# retire from the execution path without taking the evidence with it.
#
# The check lives as a test rather than a program because it needs Ronin's
# graph internals to compare on. It changes the process working directory, so
# ordinary parallel libtest runs ignore it and this script runs it alone.

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

corpus_size=$(find kati/testcase -name '*.mk' 2>/dev/null | wc -l)
if [ "$corpus_size" -eq 0 ]; then
    echo "make-equivalence: kati/testcase holds no makefiles; the submodule is not checked out." >&2
    echo "Run: git submodule update --init" >&2
    exit 1
fi

exec cargo test --release --lib \
    make::equivalence::the_direct_graph_matches_the_manifest_over_the_corpus \
    -- --ignored --exact --test-threads=1 --nocapture
