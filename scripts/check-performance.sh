#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# `.cargo/config.toml` names the host as an explicit target, so cargo's
# artifacts sit under the triple rather than directly under `target/`. Ask
# rustc for it rather than spelling it a second time.
release=target/$(rustc -vV | sed -n 's/^host: //p')/release

cargo build --release --bin ronin --example baseline
# The example defaults to a profile path that naming the target moved, so
# the script that knows where the build put things says so.
exec $release/examples/baseline --ronin "$repo_root/$release/ronin" --validate "$@"
