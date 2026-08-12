#!/bin/sh
# Stand in for a distribution's Make so the corpus can be measured against it.
#
# Builds of GNU Make 4.4.1 disagree — Debian's answers `-rvU` where the
# released source answers `-rv` — and the only way to find out where else they
# disagree is to run the corpus against each of them. They cannot be installed
# beside one another, so they arrive as containers and this is the command that
# stands in for one:
#
#   MAKE_ORACLE_IMAGE=fedora MAKE_PORT_COMPARE=1 \
#     MAKE_PORT_ORACLE=$PWD/scripts/make-oracle-container.sh \
#     cargo test --test make_port -- make_build_intent_matches_oracle --nocapture
#
# The case directory is mounted at the path it already holds, so a Makefile
# naming an absolute path finds the same file inside, and mtimes are the host's
# because the files are. The container runs as the invoking user, so what a
# recipe writes belongs to whoever reads it afterwards rather than to root.
#
# `latest` rather than a digest: which build a distribution currently ships is
# the question being asked, and the answer is dated by the run that recorded
# it. What was measured is written down in docs/make-oracle-divergences.md.
set -eu

image=${MAKE_ORACLE_IMAGE:?MAKE_ORACLE_IMAGE names the distribution to run}
tag=ronin-make-oracle-$image

if ! docker image inspect "$tag" >/dev/null 2>&1; then
    case $image in
        fedora) install="dnf -y install make && dnf clean all" ;;
        archlinux) install="pacman -Sy --noconfirm make && pacman -Scc --noconfirm" ;;
        *)
            echo "make-oracle-container: no recipe for $image." >&2
            exit 1
            ;;
    esac
    printf 'FROM %s:latest\nRUN %s\n' "$image" "$install" |
        docker build -q -t "$tag" - >&2
fi

directory=$(pwd)
exec docker run --rm \
    --volume "$directory:$directory" \
    --workdir "$directory" \
    --user "$(id -u):$(id -g)" \
    --env LC_ALL=C \
    "$tag" make "$@"
