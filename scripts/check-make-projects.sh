#!/bin/sh
# Build vim and zsh from their own Makefiles with Ronin's Make mode.
#
# check-make-project.sh already builds Ninja's CMake-generated Makefile, and
# that gate is worth what it costs: it found the jobserver not reaching a
# recursive sub-make. But a generated Makefile is a Makefile one program wrote
# for one purpose, and it exercises the shapes that program emits and no
# others. vim and zsh are hand-written build systems that have been maintained
# for thirty years, and they use recursion in ways a generator never would:
#
#   vim   the top-level Makefile's only job is to hand every goal to src/. One
#         recipe holds a liftable `cd src && $(MAKE) $@` beside two dead-branch
#         guards that each hold `$(MAKE)` calls the compiler cannot lift. GNU
#         runs the guards, they are false, and nothing recursive happens.
#
#   zsh   Src/Makefile dispatches ten targets into a *generated* Makemod with
#         `@$(MAKE) -f Makemod $(MAKEDEFS) $@`, Makemod re-invokes itself for
#         `X.mdh.tmp`, and the module objects are built by a suffix rule whose
#         target suffix is `..o` — two dots, and `.SUFFIXES` says so.
#
# Neither is reachable from the conformance corpus: every case in it is a
# Makefile written to isolate one feature, and these are the ways features
# combine.
#
# The sources are pinned tarballs with published checksums, fetched the way
# build-make-oracle.sh fetches GNU Make, and left in reference/ — large,
# host-specific, rebuildable, and gitignored. Configuring is autoconf's work,
# not Make's, so it is done once and kept; rerunning this measures Make.
#
# Usage: scripts/sandboxed scripts/check-make-projects.sh [vim|zsh]
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# `.cargo/config.toml` names the host as an explicit target, so cargo's
# artifacts sit under the triple rather than directly under `target/`. Ask
# rustc for it rather than spelling it a second time.
release=target/$(rustc -vV | sed -n 's/^host: //p')/release

home=$repo_root/reference/make-projects
mkdir -p "$home"

vim_version=9.2.0957
vim_checksum=709eae992e6f453ca428049677a9538d9ea9a2ab48104a6e750d82e279075764
vim_url=https://github.com/vim/vim/archive/refs/tags/v$vim_version.tar.gz

zsh_version=5.9.2
zsh_checksum=36fa734374b44783582cec09bcd67822e2f992c779ec1624ab5596df078d2f81
zsh_url=https://www.zsh.org/pub/zsh-$zsh_version.tar.xz

fetch() {
    tarball=$1
    url=$2
    checksum=$3
    if [ ! -f "$tarball" ]; then
        echo "check-make-projects: fetching $url"
        if command -v curl >/dev/null 2>&1; then
            curl -fsSL -o "$tarball.part" "$url"
        elif command -v wget >/dev/null 2>&1; then
            wget -q -O "$tarball.part" "$url"
        else
            echo "check-make-projects: neither curl nor wget is installed." >&2
            exit 1
        fi
        mv "$tarball.part" "$tarball"
    fi
    observed=$(sha256sum "$tarball" | cut -d' ' -f1)
    if [ "$observed" != "$checksum" ]; then
        echo "check-make-projects: $tarball hashes to $observed," >&2
        echo "but the pinned release is $checksum." >&2
        echo "Delete it and rerun to fetch again." >&2
        exit 1
    fi
}

cargo build --release --bin ronin

# Make mode is reached by the invoked name and by nothing else.
bin=$repo_root/target/make-projects-bin
rm -rf "$bin"
mkdir -p "$bin"
ln -s "$repo_root/$release/ronin" "$bin/make"

jobs=${JOBS:-8}

build_vim() {
    tarball=$home/vim-$vim_version.tar.gz
    source=$home/vim-$vim_version
    fetch "$tarball" "$vim_url" "$vim_checksum"
    if [ ! -f "$source/src/configure.ac" ]; then
        rm -rf "$source"
        tar -x -z -f "$tarball" -C "$home"
    fi
    if [ ! -f "$source/src/auto/config.mk" ]; then
        (
            cd "$source/src"
            # Everything optional is named rather than detected: a build that
            # links whatever the host happens to have installed is a build that
            # measures the host.
            ./configure \
                --with-features=huge \
                --enable-multibyte \
                --disable-gui \
                --without-x \
                --disable-nls \
                --enable-pythoninterp=no \
                --enable-python3interp=no \
                --enable-perlinterp=no \
                --enable-luainterp=no \
                --enable-rubyinterp=no \
                --enable-tclinterp=no \
                >configure.log 2>&1 || { tail -20 configure.log >&2; exit 1; }
        )
    fi

    # From the top, which is the point: the recipe that hands every goal to
    # src/ is the one that mixes a liftable recursion with two it cannot lift.
    cd "$source"
    "$bin/make" clean >/dev/null 2>&1 || true
    start=$(date +%s)
    "$bin/make" -j"$jobs" >/dev/null
    finish=$(date +%s)

    # Reporting success is not the same as having worked, and a front end that
    # quietly skipped a link would report success.
    if [ ! -x "$source/src/vim" ]; then
        echo "check-make-projects: src/vim was not built" >&2
        exit 1
    fi
    version=$("$source/src/vim" --version | head -1)
    echo "built $version from its own Makefile in $((finish - start))s at -j$jobs"
}

build_zsh() {
    tarball=$home/zsh-$zsh_version.tar.xz
    source=$home/zsh-$zsh_version
    fetch "$tarball" "$zsh_url" "$zsh_checksum"
    if [ ! -f "$source/configure" ]; then
        rm -rf "$source"
        tar -x -J -f "$tarball" -C "$home"
    fi
    if [ ! -f "$source/config.status" ]; then
        (
            cd "$source"
            ./configure --disable-gdbm --enable-multibyte \
                >configure.log 2>&1 || { tail -20 configure.log >&2; exit 1; }
        )
    fi

    cd "$source"
    "$bin/make" clean >/dev/null 2>&1 || true
    start=$(date +%s)
    # zsh's own Makefiles declare .NOTPARALLEL, so -j is the parent's to offer
    # and the Makefile's to decline; passing it is what the sandbox does.
    "$bin/make" -j"$jobs" >/dev/null
    finish=$(date +%s)

    if [ ! -x "$source/Src/zsh" ]; then
        echo "check-make-projects: Src/zsh was not built" >&2
        exit 1
    fi
    # The modules are the half of zsh that the `..o` suffix rule builds, and a
    # binary with none of them links and runs and is not zsh. Asking for the
    # count rather than for one name keeps this from passing on a stub.
    modules=$(find "$source/Src" -name '*.so' | wc -l)
    if [ "$modules" -lt 20 ]; then
        echo "check-make-projects: only $modules modules built, expected 20+" >&2
        exit 1
    fi
    version=$("$source/Src/zsh" --version | head -1)
    echo "built $version and $modules modules from its own Makefile in $((finish - start))s at -j$jobs"
}

case "${1:-all}" in
    vim) build_vim ;;
    zsh) build_zsh ;;
    all)
        (build_vim)
        (build_zsh)
        ;;
    *)
        echo "usage: $0 [vim|zsh|all]" >&2
        exit 1
        ;;
esac
