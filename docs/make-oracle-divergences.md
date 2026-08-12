# Where builds of GNU Make 4.4.1 disagree

Four programs print `GNU Make 4.4.1`: the source the Free Software Foundation
released, Debian's `make-dfsg 4.4.1-2`, Fedora's build, and Arch's. They are
not the same program. Ronin's build-intent corpus under `tests/make` is
recorded from one of them, and until the record described below existed it
could not say which — so a re-record on another host would have overwritten one
distribution's answers with another's without a word.

This document is what each of them answers differently, and how the corpus was
classified when the oracle moved to the released source.
[`[spec:ronin:req:make.oracle-provenance]`](spec/ronin/make.md).

## The oracle

Upstream GNU Make 4.4.1, built from the release tarball:

| | |
| --- | --- |
| Source | `https://ftp.gnu.org/gnu/make/make-4.4.1.tar.gz` |
| `sha256` | `dd16fb1d67bfab79a72f5e8390735c49e3e8e70b4945a15ab1f81ddb78658fb3` |
| Built by | `scripts/build-make-oracle.sh` |
| Binary | `reference/make-oracle/make-4.4.1/make` (gitignored, like the pinned Ninja) |

The tarball rather than `reference/gnumake`, which is the same release as a git
checkout — tag 4.4.1, commit `d66a65a`, the commit `scripts/check-make-upstream.sh`
pins — but carries no generated `configure`. Bootstrapping it wants a gnulib
clone at a revision the checkout does not vendor; the tarball is that source
with `configure` already in it.

`tests/make/oracle.provenance` is the corpus's record of which Make made it:
the reported version, the host it reports being built for, every variable it
installs at `default` origin, the values `.POSIX:` changes, and the features it
offers. Recording refuses when the Make in front of it answers differently, so
moving the oracle is an edit to that record — `MAKE_PORT_ORACLE_MOVE` — rather
than a silent overwrite.

Between the four builds, three lines of that record do the discriminating:

| Build | How the record tells it apart |
| --- | --- |
| upstream 4.4.1 | none of the below |
| Debian `make-dfsg 4.4.1-2` | `posix ARFLAGS -rvU` |
| Fedora | `host x86_64-redhat-linux-gnu` |
| Arch | `feature guile` |

Nothing else in the catalogue of built-in variables differs between them.

## Running the corpus against another build

```
MAKE_ORACLE_IMAGE=fedora MAKE_PORT_COMPARE=1 \
  MAKE_PORT_ORACLE=$PWD/scripts/make-oracle-container.sh \
  cargo test --test make_port -- make_build_intent_matches_oracle --nocapture
```

Comparison mode runs each case with the named Make and reports how it differs
from the recording; it never writes one. `scripts/make-oracle-container.sh`
stands in for a Make that cannot be installed beside the host's, mounting the
case directory at the path it already has so that mtimes stay the host's and a
Makefile naming an absolute path finds the same file. Everything else in the
run — `/bin/sh`, the shell utilities a recipe calls — is the container's, which
is the point: a difference can be the Make or it can be the userland, and the
two distributions are run so the answer can be told from whether they agree
with each other.

## Debian `make-dfsg 4.4.1-2`

Debian 13, `/usr/bin/make`, the build the corpus was recorded from before the
oracle moved. It was measured by re-recording the whole corpus from it under
the provenance-carrying harness and re-recording again from upstream, so the
comparison is recording against recording rather than a judgement about which
cases to look at. Two runs from the same build were byte-identical, so nothing
below is a flake.

**Identity.** One departure: `ARFLAGS` is `-rvU` under `.POSIX:` where the
released source says `-rv` (`src/read.c`, `check_specials`). Debian carries a
patch asking `ar` for a non-deterministic archive — `U` turns off `ar`'s
deterministic mode, restoring the mtimes, uids and modes that mode omits. The
ordinary, non-POSIX `ARFLAGS` is `-rv` in both. Nothing else in the built-in
catalogue, the feature list or the reported host differs.

**Corpus.** One case of 324:

| Case | Moved | Class |
| --- | --- | --- |
| `target-posix-variable-defaults` | `ARFLAGS=-rvU` → `ARFLAGS=-rv` | distribution patch |

No case moved for any other reason. In particular nothing moved for a host
reason — both builds ran on the same host against the same `/bin/sh` and the
same tools — so the re-record has no host-environment class at all. The
remaining 271 changed files in that commit are the recording format: the
harness had already replaced a numeric `status` line with `outcome`, and cases
recorded before that change carried the old spelling until every case was
re-recorded at once.

**What Ronin does.** `kati/src-rs/builtins.rs` used to install `-rvU` under
`.POSIX:` to match the recording, with the departure noted in a comment. It now
installs `-rv`, which is GNU's.

The cost is real and worth naming. On a host whose `ar` defaults to
deterministic mode — Debian's does, which is why the patch exists — `-rv`
writes member headers with a zeroed date, and an archive member whose date is
the epoch is older than every source, so it is out of date on every build.
Measured here, three consecutive builds of a `.POSIX:` archive rule ran the
`ar` recipe three times under `-rv` and once under `-rvU`. The failure is
over-building rather than under-building: nothing goes stale, work is repeated.
That is what upstream GNU Make does on this host, and matching it is the
position taken — the corpus records what the released source does, and a
distribution's workaround for its own toolchain is that distribution's answer,
written down here rather than implemented.

## Fedora 44

`make-4.4.1-12.fc44.x86_64`, from `fedora@sha256:6c75d5bf57cb0fa5aa4b92c6a83c8`
`6c791644496d9ac230de7711f5b8ec3b898`, run 2026-08-12. Reports `GNU Make 4.4.1`.

**Identity.** One departure, and it is not behavioural: `MAKE_HOST` is
`x86_64-redhat-linux-gnu` rather than `x86_64-pc-linux-gnu`, which is the
configure triple the package was built with. Every built-in variable, including
`ARFLAGS` under `.POSIX:`, and every entry in `.FEATURES` matches the released
source.

**Corpus.** 0 of 324 cases differ — with Fedora's `/bin/sh` and Fedora's shell
utilities as well as Fedora's Make. Fedora ships the released behaviour.

## Arch

`make 4.4.1-3`, from `archlinux@sha256:b0deabeb3d283da2c7f7dbf0eea051b7b2cd0554`
`e0b737cc457fd21683bdcdd1`, run 2026-08-12. Reports `GNU Make 4.4.1` and the
same host triple the released source reports, so `MAKE_HOST` does not tell it
apart from upstream.

**Identity.** One departure: `.FEATURES` carries `guile`, so the package is
built `--with-guile` and offers the `$(guile ...)` function the corpus's oracle
does not have. Every built-in variable, `ARFLAGS` under `.POSIX:` included,
matches the released source.

**Corpus.** 0 of 324 cases differ, again with the distribution's own userland.
No case reaches `$(guile ...)`, so the extra function is a capability the
corpus never asks about rather than a behavioural difference within it.

## Summary

| | Debian 13 | Fedora 44 | Arch | upstream |
| --- | --- | --- | --- | --- |
| Package | `make 4.4.1-2` | `make-4.4.1-12.fc44` | `make 4.4.1-3` | tarball |
| `--version` | `GNU Make 4.4.1` | `GNU Make 4.4.1` | `GNU Make 4.4.1` | `GNU Make 4.4.1` |
| `MAKE_HOST` | `x86_64-pc-linux-gnu` | `x86_64-redhat-linux-gnu` | `x86_64-pc-linux-gnu` | `x86_64-pc-linux-gnu` |
| `.POSIX:` `ARFLAGS` | `-rvU` | `-rv` | `-rv` | `-rv` |
| `guile` feature | no | no | yes | no |
| Cases differing from the recording | 1 of 324 | 0 of 324 | 0 of 324 | — |

Debian is the odd one out, and it is the build the corpus was recorded from.
The single case it moves is the one this whole exercise was worth doing for:
without the record, a re-record on Fedora or Arch would have silently replaced
`ARFLAGS=-rvU` with `ARFLAGS=-rv` and nobody would have been told which of the
two was GNU's.
