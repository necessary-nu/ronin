#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# `.cargo/config.toml` names the host as an explicit target, so cargo's
# artifacts sit under the triple rather than directly under `target/`. Ask
# rustc for it rather than spelling it a second time.
release=target/$(rustc -vV | sed -n 's/^host: //p')/release

# The gate measures the Make evaluator against GNU Make, so it runs the fork's
# own binary: what a Makefile evaluates to is decided before any front end
# builds it, and rkati is that evaluation with nothing else on top.
#
# Ronin's Make mode is the same evaluation followed by Ronin's scheduler, and
# the corpus can be pointed at it. Make mode is reached by the invoked name and
# by nothing else, so that means a make-named link rather than a flag:
#
#   ln -sf "$PWD/$release/ronin" /tmp/ronin-make/make
#   scripts/check-make-conformance.sh --front-end /tmp/ronin-make/make
#
# That is not the gate, and the run says why: Ronin prints its own progress
# line where Make echoes each recipe, so nearly every case differs on that one
# line and on nothing else. Retargeting means reclassifying the corpus against
# a second contract, which belongs to whoever owns that decision rather than to
# whoever passes the flag.
# The oracle is upstream 4.4.1 as the Free Software Foundation released it, not
# whichever build of 4.4.1 the host has: Debian's, Fedora's and Arch's all print
# the same version string and do not all answer the same questions. The harness
# defaults to the binary this leaves behind and refuses to classify the corpus
# against anything the record in tests/make/oracle.provenance does not describe,
# so building it here is what makes the gate run without a host Make at all.
#
# Idempotent: with the binary already built this only re-checks the tarball's
# checksum.
scripts/build-make-oracle.sh >/dev/null

cargo build --release -p kati --bin rkati
cargo build --release --bin ronin
cargo build --release --example make_conformance

# The example defaults to a profile path that naming the target moved, so
# the script that knows where the build put things says so.
exec $release/examples/make_conformance --front-end "$release/rkati" "$@"
