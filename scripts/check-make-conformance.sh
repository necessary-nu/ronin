#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# The Make front end has no command line of its own yet, so the corpus runs
# against the fork's own binary. Retargeting to Ronin's Make mode is
# --front-end target/release/ronin plus whatever selects that mode.
cargo build --release -p kati --bin rkati
cargo build --release --example make_conformance

exec target/release/examples/make_conformance "$@"
