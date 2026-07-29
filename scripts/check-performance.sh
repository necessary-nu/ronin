#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

cargo build --release --bin ronin --example baseline
exec target/release/examples/baseline --validate "$@"
