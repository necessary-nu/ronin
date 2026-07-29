#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

cargo test --all-targets
cargo build --release --bin ronin --example conformance
exec target/release/examples/conformance "$@"
