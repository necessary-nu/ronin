#!/bin/sh
# Every lint gate this repository has, in one spelling.
#
# It exists because the spelling used to live in two places — this repository's
# release script and each dispatch's habit — and they disagreed for weeks. The
# release script said one thing about three bodies of code in one command and
# therefore reached none of them; the campaign ran a hand-typed split beside it;
# and a plain `cargo clippy --all-targets`, which anyone might reasonably try,
# was red on main for findings nobody had been shown. There is one statement of
# the standard now, and both the release gate and a dispatch run this file.
#
# TWO passes, where there were three. The middle pass used to take the pedantic
# and nursery groups off Ronin's tests, examples and benches, which is why a
# separate `--lib --bins` pass was needed to hold the product code to the house
# standard. The findings that forced that split are gone
# (make-release-pedantic-in-the-test-scope), so `--all-targets` now says what
# the two of them used to say between them and the narrower one is subsumed.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# Ronin's own code — lib, bins, tests, examples and benches — at the standard
# `[lints.clippy]` in Cargo.toml declares.
#
# The groups are NOT repeated on the command line, and that is the point rather
# than an omission. `-W clippy::nursery` here would not restate the standard, it
# would override it: the package deliberately allows `redundant_pub_crate`, and
# a command-line `-W` on the whole group turns that allow back on and produces
# seventeen findings in examples/support/workloads.rs which are a decision being
# overridden rather than anything wrong with the code. Cargo.toml is where the
# standard is stated, `-D warnings` is what makes the gate fail on it, and the
# consequence is that a bare `cargo clippy` in a terminal and this gate agree.
#
# `--no-deps` keeps the run to Ronin's own crate; the Make front end has its own
# pass below.
cargo clippy --all-targets --no-deps -- -D warnings

# The Make front end, separately: it is a vendored fork, and rewriting Google's
# code to Ronin's house style is not this gate's business. Plain `-D warnings`
# is, and it is what the fork's own `#![deny(warnings)]` used to do — moved here
# so that a rustc release which adds a lint fails a check someone is running
# rather than every build of the tree.
cargo clippy -p kati --all-targets -- -D warnings
