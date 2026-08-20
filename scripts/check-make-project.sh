#!/bin/sh
# Build a real project's real Makefile with Ronin's Make mode.
#
# GNU Make's test suite asks whether a construct behaves; this asks the only
# question the product is for. The suite cannot answer it: every case in it is
# a Makefile written to isolate one feature, and the ways a real build breaks
# are the ways features combine. This gate found the jobserver not reaching a
# recursive sub-make, which the suite scores as nothing at all.
#
# The subject is Ninja's own CMake build, generated as Unix Makefiles: 541
# recursive `$(MAKE)` lines, a `.NOTPARALLEL`, generated dependency files, and
# a hundred-odd compiles. It is not vendored — reference/ninja is the pinned
# checkout the Ninja conformance gate already needs, and this reuses it rather
# than asking for a second copy.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# `.cargo/config.toml` names the host as an explicit target, so cargo's
# artifacts sit under the triple rather than directly under `target/`. Ask
# rustc for it rather than spelling it a second time.
release=target/$(rustc -vV | sed -n 's/^host: //p')/release

source=$repo_root/reference/ninja
build=$source/build-make
if [ ! -f "$source/CMakeLists.txt" ]; then
    echo "check-make-project: no Ninja source at $source." >&2
    echo "It is the same checkout check-ninja-conformance.sh needs." >&2
    exit 1
fi
if ! command -v cmake >/dev/null 2>&1; then
    echo "check-make-project: cmake is needed to generate the Makefile." >&2
    exit 1
fi

cargo build --release --bin ronin

# Make mode is reached by the invoked name and by nothing else.
bin=$repo_root/target/make-project-bin
rm -rf "$bin"
mkdir -p "$bin"
ln -s "$repo_root/$release/ronin" "$bin/make"

# Generated once and kept: configuring is CMake's work, not Make's, and doing
# it every run would measure the wrong tool.
if [ ! -f "$build/Makefile" ]; then
    cmake -S "$source" -B "$build" -G "Unix Makefiles" -DCMAKE_BUILD_TYPE=Release
fi

jobs=${JOBS:-8}
cd "$build"
"$bin/make" clean >/dev/null 2>&1 || true

start=$(date +%s)
"$bin/make" -j"$jobs" >/dev/null
finish=$(date +%s)

# The build reporting success is not the same as the build having worked, and
# a Makefile front end that quietly skipped a link would report success.
for artefact in ninja ninja_test; do
    if [ ! -x "$artefact" ]; then
        echo "check-make-project: $artefact was not built" >&2
        exit 1
    fi
done
version=$("./ninja" --version)

echo "built ninja $version from its own Makefile in $((finish - start))s at -j$jobs"
