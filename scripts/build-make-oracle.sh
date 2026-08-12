#!/bin/sh
# Build the Make the build-intent corpus is recorded against.
#
# A distribution's `make` prints "GNU Make 4.4.1" whether or not it is the
# program the Free Software Foundation released under that name. Debian's
# make-dfsg 4.4.1-2 answers `-rvU` where the released source answers `-rv`, and
# a corpus recorded from it says GNU Make where it means Debian. So the oracle
# is built here, from the release the FSF signed, and the corpus records what
# that build answers — see [spec:ronin:req:make.oracle-provenance].
#
# The tarball rather than reference/gnumake, which is a git checkout with no
# generated `configure`: bootstrapping it wants a gnulib clone at a revision the
# checkout does not carry, and the release tarball is the same source with
# `configure` already in it. The checksum is the FSF's published one, so the
# build starts from a byte the release can be checked against rather than from
# whatever the mirror served.
#
# The result is gitignored, like the pinned Ninja beside it: large, host
# specific, rebuildable by rerunning this, and kept in the checkout rather than
# /tmp, which a reboot empties without telling anyone.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

version=4.4.1
checksum=dd16fb1d67bfab79a72f5e8390735c49e3e8e70b4945a15ab1f81ddb78658fb3
url=https://ftp.gnu.org/gnu/make/make-$version.tar.gz

home=$repo_root/reference/make-oracle
tarball=$home/make-$version.tar.gz
source=$home/make-$version
oracle=$source/make

mkdir -p "$home"

if [ ! -f "$tarball" ]; then
    echo "build-make-oracle: fetching $url"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$tarball.part" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$tarball.part" "$url"
    else
        echo "build-make-oracle: neither curl nor wget is installed." >&2
        exit 1
    fi
    mv "$tarball.part" "$tarball"
fi

observed=$(sha256sum "$tarball" | cut -d' ' -f1)
if [ "$observed" != "$checksum" ]; then
    echo "build-make-oracle: $tarball hashes to $observed," >&2
    echo "but GNU Make $version is $checksum." >&2
    echo "Delete it and rerun to fetch again." >&2
    exit 1
fi

if [ ! -x "$oracle" ]; then
    rm -rf "$source"
    tar -x -z -f "$tarball" -C "$home"
    (
        cd "$source"
        ./configure --disable-dependency-tracking >configure.log 2>&1 ||
            { tail -20 configure.log >&2; exit 1; }
        make -s >build.log 2>&1 || { tail -20 build.log >&2; exit 1; }
    )
fi

reported=$("$oracle" --version | head -1)
if [ "$reported" != "GNU Make $version" ]; then
    echo "build-make-oracle: the build reports '$reported', not 'GNU Make $version'." >&2
    exit 1
fi

echo "$oracle"
