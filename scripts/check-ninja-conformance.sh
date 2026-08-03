#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

cargo test --all-targets

# Ronin has no Windows host to run on, which is exactly why the Windows code
# path stops compiling without anyone noticing — it had, in two places. Type
# checking it costs seconds and catches that; it says nothing about behaviour.
windows_target=x86_64-pc-windows-gnu
if rustup target list --installed 2>/dev/null | grep -qx "$windows_target"; then
    cargo check --target "$windows_target" --lib
else
    echo "conformance: skipping the Windows type check ($windows_target not installed)" >&2
fi

cargo build --release --bin ronin --example conformance
exec target/release/examples/conformance "$@"
