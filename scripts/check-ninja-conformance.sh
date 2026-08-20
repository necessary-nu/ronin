#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# `.cargo/config.toml` names the host as an explicit target, so cargo's
# artifacts sit under the triple rather than directly under `target/`. Ask
# rustc for it rather than spelling it a second time.
release=target/$(rustc -vV | sed -n 's/^host: //p')/release

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
# The example defaults to a profile path that naming the target moved, so
# the script that knows where the build put things says so.
exec $release/examples/conformance --ronin "$repo_root/$release/ronin" "$@"
