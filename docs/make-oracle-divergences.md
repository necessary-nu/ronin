# Where builds of GNU Make 4.4.1 disagree

Four programs print `GNU Make 4.4.1`: the source the Free Software Foundation
released, Debian's `make-dfsg 4.4.1-2`, Fedora's build, and Arch's. They are
not the same program. Ronin's build-intent corpus under `tests/make` is
recorded from one of them, and until the record described below existed it
could not say which — so a re-record on another host would have overwritten one
distribution's answers with another's without a word.

This document is what each of them answers differently, and how Ronin's two
Make corpora — the build-intent one under `tests/make` and the vendored kati
one under `kati/testcase` — were classified when the oracle moved to the
released source.
[`[spec:ronin:req:make.oracle-provenance]`](spec/ronin/make.md).

The last section is about a fifth kind of disagreement — Ronin against all four
builds of GNU Make. It began as a survey for the operator to rule on. On
2026-08-24 he ruled on **every one of its ten numbered sections** — three of them
no longer divergences at all, and one whose conditional ruling failed its
condition and is therefore filed as a defect rather than accepted:
[where Ronin diverges from the oracle](#where-ronin-diverges-from-the-oracle).

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

There is one such record because there is one oracle. Both gates that need a
Make read it through `tests/support/oracle.rs`: `tests/make_port.rs` when it
records the build-intent corpus, and `examples/make_conformance.rs` before it
classifies the vendored kati corpus. The third consumer,
`scripts/check-make-upstream.sh`, needs none — its oracle is GNU Make's own
test suite at commit `d66a65a`, a Perl driver it hands Ronin and nothing else;
it never runs a Make.

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

## The vendored kati corpus

`examples/make_conformance.rs` runs the 387 cases under `kati/testcase` twice
and classifies every difference in `tests/make_corpus_inventory.tsv`. It used
to resolve its Make from `PATH` and admit it on the version string alone —
exactly the weakness the record above exists to close — so the inventory was
recorded against Debian's build. It now defaults to the binary
`scripts/build-make-oracle.sh` leaves behind, checks it against the record
before a single case runs, and refuses on any departure. `--other-make` points
it at a build that is deliberately not the oracle: the departures are printed
as a header, and `--update` refuses, so no other build can rewrite the
classification.

**Oracle.** Nothing in this corpus tells Debian's build apart from upstream.
The two runs agree case for case, byte for byte, because the only behavioural
departure between them is `ARFLAGS` under `.POSIX:` and the corpus has exactly
one `.POSIX:` case — `posix_var.mk`, which declares it to observe what
`override SHELL := echo` does to `$(shell ...)` and never mentions `ARFLAGS`,
`ar` or an archive member. `-rvU` had nowhere to show.

**Name.** Ten rows moved anyway, and not one of them for a reason about a build
of Make. The corpus reads the name of the tool it is handed. Seven scripts —
`final_global`, `final_rule`, `final_rule2`, `readonly_global`,
`readonly_global_missing`, `readonly_rule`, `readonly_rule_missing` — test it
with `grep -q "^make"` and print a canned expectation *instead of* running the
tool when it matches. Three makefiles — `err_export_override.mk`,
`err_override_export.mk`, `wildcard_cache.mk` — hold `ifeq
($(MAKE)$(MAKEVER),make4)` and answer `$(error test skipped)`. Both tests match
the bare word `make` and neither matches a path, so under the old default GNU
Make described itself in ten cases and ran in none of them:

| Cases | Was | Is |
| --- | --- | --- |
| 3 makefiles | GNU Make skipped itself; kati ran | both run, and agree — rows removed |
| 7 scripts | kati against the corpus's canned Make text | kati against a GNU Make that ran |

`final_rule.sh` is the shape of it. Handed `make`, the script prints
`Makefile:3: *** cannot assign to readonly variable: FOO` and exits 0 without
starting anything; handed a path, GNU Make runs and says `Nothing to be done
for 'all'`, which is what it actually does with kati's `$=` final-assignment
syntax. The recorded difference had been kati's `  Stop.` suffix against a
sentence the corpus wrote for it. The seven stay `extension` — the feature is
still kati's alone — but their Make side is now evidence rather than opinion.

**The corpus's own Make.** Twelve makefiles compute `MAKEVER` from `$(shell
make --version)` and branch on it — `comment_in_command`, `include_glob_order`,
`posix_var`, `multi_implicit_output_patterns`, `multiline_recipe`,
`implicit_pattern_rule_prefix`, `var_with_space`, `wildcard`,
`shell_var_with_args`, `err_export_override`, `err_override_export` and
`wildcard_cache`. That `make` is a bare name, so it used to be the host's,
whichever oracle was passed. It was symmetric — both tools evaluated the same
`$(shell ...)` and took the same branch — and therefore tilted nothing; what it
did was let a program the gate does not identify choose what twelve cases test.

The harness now puts a `make` link to the pinned oracle in front of `PATH` for
both runs, so the corpus asks the build the record names. Measured rather than
argued: with a `make` on `PATH` that answers `GNU Make 3.81`, the gate before
this went 318/69 → 317/70 and refused with an unclassified difference in
`shell_var_with_args.mk#test`; with the link it is 318/69 and green, identical
to a run on a host whose `make` is 4.4.1. Neither tool is reached through the
link — both are spawned by absolute path, so `$(MAKE)` is still the path each
was invoked with, the seven `^make` scripts still see a path, and
`submake_basic.mk` still recurses into the tool under test.

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

## Where Ronin diverges from the oracle

Everything above is one build of GNU Make against another. This section is Ronin
against all four of them: every place a Ronin `make` build observably differs
from GNU Make 4.4.1, gathered so the operator can read one document and rule on
each — which, for all ten, he now has.

It began as a survey. The operator's instruction of 2026-08-24, verbatim: *"I do
not think we have any accepted divergences. You will need to explain all the
divergences now."* The survey was then explained to him, and on that date he
ruled on seven of the ten numbered sections — #2, #3, #5, #7, #8, #9 and #10.
Later the same day he ruled on the two he had left — **#1** (*"leave it."*) and
**#6** (*"sounds like a bug in GNU, fuck GNU."*) — and confirmed the SIGQUIT
carve-out inside #5 with the word *"exit 1"*. **#4** is the tenth, and his ruling
on it is not a sentence about it: its only trigger was `.KATI_DEPFILE`, and on
the same day he ruled *"fuck kati extensions"* — remove them from the product. A
divergence whose entry point no makefile can name is not one to accept or to
repair; it is one that goes with the surface, and it did, the same day.

So **every numbered section below carries an operator ruling**, each quoted
verbatim with its date on the section's **Authority** line. That is a statement
about authorship, not about approval: **three** of the ten (**#4**, **#5**, **#7**)
are no longer divergences at all — two ruled *closed* with the code changed to
match GNU Make, and one whose trigger was deleted with the kati extensions — and
one (**#8**) is not accepted either: his ruling on it was *conditional*, the
condition was measured, and it failed, so #8 is filed as a defect to fix.

How it got here is worth keeping, because it is the reason the document exists.
Until 2026-08-24 this section carried the sentence *"nothing below should be read
as approved"*, and it was true: every disposition here had been recorded by the
delivering dispatch — authored under the operator's git identity, but written by
the agent — with no standalone operator ruling behind any of it. That sentence is
retired because it stopped being true, one ruling at a time, and not because the
standard for it moved.

Every measured cell below was re-measured against `reference/make-oracle/make-4.4.1/make`
on 2026-08-24.

### Summary

| # | Divergence | Reachable in a real build? | Gated by | Authority |
| --- | --- | --- | --- | --- |
| 1 | A shuffled order-only prerequisite is permuted apart from the rest | No — only under `--shuffle`, plus one `$|` cell needing a cycle | 2 `divergence` sidecars in `make_port` | **operator, 2026-08-24**: *"leave it."* |
| 2 | `$(MAKEFLAGS)` hands back the text it stores | Only where a switch value holds a literal `$` | 2 `divergence` sidecars + this doc | **operator, 2026-08-24**: *"We keep our behaviour."* |
| 3 | An oversized recipe's marked lines do not run under `-t`/`-q` | No — needs a recipe line over 100 kB | `tests/shell.rs::oversized_marks_are_not_split_out` | **operator, 2026-08-24**: *"lol not a real problem."* |
| 4 | ~~A `.KATI_DEPFILE` recipe's `$(file …)` runs where it is built~~ **GONE** | was No — `.KATI_DEPFILE` was a kati extension no GNU makefile names | the extension is removed, so nothing reaches it | **operator, 2026-08-24**: *"fuck kati extensions"* — the trigger went, and the divergence went with it |
| 5 | ~~An interrupt leaves Ninja's 130, not GNU's 128+signum~~ **FIXED** | was **Yes** | `tests/interrupts.rs`; `product.build-outcome+1` | **operator, 2026-08-24**: *"I think ronin should actually follow the GNU case even for ninja."* |
| 6 | `-n` does not run a `+`-marked or `$(MAKE)`-referencing line | Only under `-n` with such a line | `DISCOVERY_ONLY_CASES` in `make_port` | **operator, 2026-08-24**: *"sounds like a bug in GNU, fuck GNU."* |
| 7 | ~~An output's directory is created where GNU leaves it to the recipe~~ **FIXED** | was **Yes** | 4 corpus fixtures + a kati unit pair | **operator, 2026-08-24**: *"Make's rule applies when run as make."* |
| 8 | Recursive keep-going choreography differs | **Yes** — recursive `$(MAKE)` with per-child `-k`/`-S` | `DISCOVERY_ONLY_CASES` in `make_port`; node `make-recursive-keep-going-choreography-writes-different-files` | **DEFECT** — his ruling was conditional and the condition FAILED (measured) |
| 9 | `-W` over a `::` chain refuses before the chain's work runs | Only under `-W`/`-t`-family over a double-colon chain | `DISCOVERY_ONLY_CASES` in `make_port` | **operator, 2026-08-24**: *"fine."* |
| 10 | `-k` builds nothing past an unmakeable prerequisite | **Yes** — `-k` with a prerequisite that has no rule | `make-keep-going-builds-what-it-can-past-an-unmakeable-prerequisite` (retired) | **operator, 2026-08-24**: *"Ronin superior. Accepted divergence."* |
| — | Two defects this survey found | **Yes** (a crash, a refused build) | filed as nodes | **none** — defects, not divergences to accept |

**On the numbering.** The operator ruled on the `-k`-past-an-unmakeable-prerequisite
divergence as **#10**, which is how it is numbered above and below. It arrived in
this document as unnumbered prose beneath a row that held two defects; the defects
were never a divergence to rule on, so they moved to the unnumbered row and the
divergence took the number he used.

A line worth stating plainly: beyond those two defects, the upstream residue held
**85 more genuinely unclassified rows**. As of 2026-08-24 all 85 have been worked
against the oracle case by case, and each one's measurement, outcome and owner is
written out in **[`make-upstream-residue.md`](make-upstream-residue.md)**. In
summary: **36 are defects** (22 nodes filed, none accepted), 23 are narration
whose reason is now a named classifier family, 7 are narration explained but not
mechanised, 4 are the jobserver handle missing from `MAKEFLAGS`, 7 turned out not
to be Ronin divergences at all (state the suite's shared `tests/` directory
carries between scripts — GNU produces Ronin's output exactly when given the same
state), 5 assert GNU's own implementation rather than Make semantics, and 3
duplicate entries already owned here or by a node. **None is undiagnosed, and
none was accepted.** The inventory records 57 unclassified rows as of the #7
delivery of 2026-08-24 — the 36 defects until they are fixed, plus the 21 that
have no honest family to move into. It held 58 until `misc/general4.diff.4`
became pure narration when Make mode stopped creating output directories; see §7.

A **twenty-third node** was filed from that work and belongs in this survey's own
list rather than in the residue document, because it is reachable from a real
build: `make-keep-going-builds-what-it-can-past-an-unmakeable-prerequisite`. It
is **§10** below, and the operator ruled it a deliberate divergence.

---

### 1. A shuffled order-only prerequisite is permuted apart from the rest — **ACCEPTED DIVERGENCE**

**Owner:** `make-shuffle-reorders-order-only-prerequisites-apart-from-the-rest`,
filed as a defect on 2026-08-24 and **retired the same day as a recorded,
operator-ruled divergence**. Full evidence and the two candidate fix shapes are
on that node. **Gated by** two `make_port` cases carrying `divergence` sidecars —
`tests/make/shuffled-order-only-prerequisites-permute-with-the-rest` and
`tests/make/a-shuffled-circular-drop-crosses-the-order-only-boundary` — so a
change that starts permuting the two groups together reopens the decision rather
than passing silently.

**What GNU does / what Ronin does.** GNU carries a target's normal and order-only
prerequisites in ONE `->next` chain (told apart per entry by `ignore_mtime`) and
shuffles that one chain. Ronin keeps two lists (`deps`, `order_onlys`) and
shuffles each within itself, so the two groups never permute across each other.

**Reproducer / measured.** `t: a b | c d` under `--shuffle=reverse`:

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| build order | `d c b a` | `b a d c` |
| `$^` / `$|` | `[a b]` / `[c d]` | same |

Only the order the four independent targets build in differs; the automatic
variables agree. **Verified reachable only under the switch:** with no
`--shuffle` and with `--shuffle=identity`, both tools build `a b c d`,
`$^=[a b]`, `$|=[c d]`.

One cell reaches an observable value, and it additionally needs a circular
order-only prerequisite. `t: a | b t c` under `--shuffle=reverse`:

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| `$|` | `[t c]` | `[b c]` |

Both drop the circular `t <- t`, both exit 0, both build the same files.

**Cost to match.** Two shapes, both of which reorder edge-minting: (A) give
`DepNode` one prerequisite list with a per-entry order-only flag (GNU's shape) —
changes the mint order for *every* Makefile the compiler touches, which the
equivalence gate and the inventory's compiler rows are pinned against; or (B) a
combined walk gated on a live shuffle — contains the blast radius to shuffled
runs at the price of two representations of one chain. The `.WAIT` barrier
(synthesised order-only edges) has to be excluded from the shuffle either way.

**Authority: operator decision, Brendan, 2026-08-24, on this section:**
*"leave it."* He read this section on that date and left it unruled; asked again
the same day, that is his answer. It is an accepted divergence rather than an
open defect, and the cost paragraph above is what he was reading when he said so:
both fix shapes reorder edge-minting, and the reachability is a switch nothing in
any corpus passes.

Both fixtures record GNU's answer from the oracle — `d c b a` for the build
order, `$| = t c` for the circular cell — so what is pinned is the difference
itself. `make_port` fails a recorded divergence that has been "repaired" as
loudly as it fails an unrecorded one. That takes the build-intent corpus from
three divergence markers to five.

### 2. `$(MAKEFLAGS)` hands back the text it stores

**Owner:** `make-makeflags-holds-its-switches-as-literal-text` (Done).
**Gated by:** two `divergence` sidecars —
`tests/make/makeflags-hands-back-the-switch-text-it-stores` and
`tests/make/a-child-keeps-a-switch-value-a-dollar-was-written-in`.

**What GNU does / what Ronin does.** GNU's `define_makeflags` doubles each `$`
and binds `MAKEFLAGS` recursively, so reading it halves the doubling again.
Ronin reads back the text it stored. Measured with `make -I 'a$b'`:

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| `$(MAKEFLAGS)` | ` -Ia$b` | ` -Ia$$b` |
| `$(value MAKEFLAGS)` | ` -Ia$$b` | ` -Ia$$b` |
| `$(MFLAGS)` | `-Ia$b` | `-Ia$b` |

What is *stored* agrees; only reading `$(MAKEFLAGS)` differs, and only where a
switch value carries a literal `$` (which is `-I` and `--debug`). The
consequence is a data loss GNU reproduces on the way to a child: GNU expands
`MAKEFLAGS` a second time in the child's `decode_env_switches`, reads `$b` as an
undefined variable, and the directory arrives as `-Ia`. Ronin passes the switch
whole.

**Reachability.** Needs a literal `$` inside a published switch value. None
exists in the build-intent corpus, the vendored kati corpus, GNU Make's own
suite, or the Makefiles of vim 9.2, zsh 5.9.2, abseil or Ninja.

**Cost to match.** Conforming on `$(MAKEFLAGS)` means reproducing GNU's
double-expansion, i.e. reproducing the data loss in the child — which the
dispatch judged is not compatibility of build intent.

**Authority: operator decision, Brendan, 2026-08-24, on this section:**
*"We keep our behaviour."* This ratifies what the delivering dispatch had
recorded on `make-makeflags-holds-its-switches-as-literal-text`, grounded in the
argument that "reproducing a data loss is not compatibility of build intent."

### 3. An oversized recipe's marked lines do not run under `-t` or `-q`

**Owner:** `an-oversized-recipes-marked-lines-cannot-be-split-out` (Done).
**Gated by:** `tests/shell.rs::oversized_marks_are_not_split_out` (generated
Makefile — no corpus case, because a 120 kB Makefile echoed back would cost a
quarter of a megabyte).

**What GNU does / what Ronin does.** GNU runs each recipe line as its own
process, so a `+`-marked line runs even under `-t`/`-q`. Ronin runs a recipe
line-by-line too, except when one line exceeds the shell argument limit: such a
line needs a response file named per edge (`<output>.rsp`), so the whole recipe
reaches one shell as one script, and one launch has one answer to "does this run
anyway" — and it is no. Measured with a `+`-marked line either side of a 120 kB
line:

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| `-t` | both marked lines run; goal touched | goal touched; neither marked line runs |
| `-q`, marked line first | marked line runs, exit 1 | exit 1, nothing written |
| `-q`, long line first | marked line does **not** run, exit 1 | agrees |
| (no switch) | whole recipe runs | agrees |

**Reachability.** Needs a recipe *line* over 100 kB that also holds a
`+`/`$(MAKE)` marked line. The longest recipe line in the build-intent corpus is
424 bytes; in the vendored kati corpus, 876 bytes; nothing in the GNU suite or
vim/zsh reaches a kilobyte.

**Cost to match.** A response file named per *step* rather than per edge — a
change to the build engine's response-file lifetime and to `take_step`, which
has to leave every Ninja edge untouched. The `-t`/`-q`-only shortcut would make
an edge's steps depend on the switch the run started with.

**Authority: operator decision, Brendan, 2026-08-24, on this section:**
*"lol not a real problem."* The reachability evidence above is what he was
reading: a recipe line over 100 kB that also carries a `+`/`$(MAKE)` line does
not exist in any corpus, and the longest line measured anywhere is 876 bytes.

### 4. A `.KATI_DEPFILE` recipe's `$(file …)` is performed where it is built — **GONE 2026-08-24**

**No longer reachable.** `.KATI_DEPFILE` was removed from the product on the
operator's ruling of the same day — *"fuck kati extensions"* — so there is no
makefile text left that puts a depfile on a Make edge, and therefore no recipe
that has to be read where it is built. Make mode now defers every recipe kind it
ever deferred, and the `$(file …)` in every one of them is performed at launch,
which is GNU's answer. See **[`make-kati-extensions.md`](make-kati-extensions.md)**.

The gate went with it, because it was gating the extension: `tests/shell.rs::a_depfile_recipe_is_read_where_built`
existed to pin both halves of the decision below and has nothing left to pin.

The rest of this section is the record of what the divergence WAS.

**Owner:** `make-recipe-file-operation-at-launch-only` (Done).
**Was gated by:** `tests/shell.rs::a_depfile_recipe_is_read_where_built` (no
corpus case — `.KATI_DEPFILE` is a kati extension, so GNU Make had no answer to
record against).

**What GNU does / what Ronin did.** GNU expands a recipe only when about to run
it, so a `$(file >)` in the recipe of an up-to-date target never happens. Ronin
holds an ordinary, `$?`, or grouped `&::` recipe unexpanded and expands it at
launch — matching GNU. A recipe naming a depfile is the exception: it is read
where it is built (the edge has to declare the depfile it will read at runtime,
and a deferred rule has no path to), so its `$(file …)` runs there even for a
current target. Measured with a `.KATI_DEPFILE` recipe whose target is current
and whose recipe holds `$(file > written,made)`:

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| `written` after the build | absent | holds `made` |

(GNU, not knowing `.KATI_DEPFILE`, treats it as an inert target-specific
variable and runs nothing.)

**Reachability.** `.KATI_DEPFILE`/`--detect_depfiles` are kati extensions;
`--detect_depfiles` is not even a switch Ronin's Make front end accepts. No GNU
makefile names either, and no corpus holds a `$(file …)` in a depfile recipe.

**Cost to match.** Wiring the runtime depfile machinery onto a deferred edge,
which the dispatch judged out of proportion to a `$(file …)` in a depfile
recipe. Deferring the recipe naively was measured to break the depfile's own
dependency (touching a listed header stopped rebuilding the target).

**Authority: operator decision, Brendan, 2026-08-24**, verbatim: *"fuck kati
extensions"* — clarified in a follow-up as: remove the kati-only extensions from
the product. That is a ruling on this section even though it never mentions it,
because `.KATI_DEPFILE` is the only text that reaches this divergence. A
divergence with no reachable trigger is not one to accept and not one to repair;
it goes when the surface goes, and the cost paragraph above stopped being a cost
at all. **Delivered the same day.** What the delivering dispatch had recorded
here — the wiring judgement — is kept above as the mechanism's rationale, not as
authority.

### 5. An interrupt leaves Ninja's 130, not GNU's 128 + signal number — **FIXED 2026-08-24**

**Ruled closed.** Operator decision, Brendan, 2026-08-24, verbatim: *"I think
ronin should actually follow the GNU case even for ninja. It seems semantically
smarter."* Asked whether that meant re-raising in BOTH modes, accepting that it
diverges from upstream Ninja, he answered yes. It is no longer a divergence from
GNU Make; it is now a **deliberate, operator-ruled divergence from upstream
Ninja**, and that is what this section records.

**Owner:** `make-mode-leaves-an-interrupt-with-2-where-both-references-say-130`
(Done) and `question-mode-answers-up-to-date-after-an-interrupt` (Done) are the
history; `an-interrupt-leaves-with-the-signal-that-caused-it` is the delivery.
**Gated by:** `tests/interrupts.rs` — `make_termination_dies_of_the_signal`,
`a_manifest_build_dies_of_the_signal_too`, `make_hangup_dies_of_the_signal`,
`make_quit_leaves_the_trouble_status_without_dumping`, and the nine cases that
already read an interrupt's status — twelve of them through the shared `died_of`
helper, which reads the wait status as a signal because `code()` is `None` for a
process a signal killed.

**What GNU does.** It catches a fatal signal, kills its children, withdraws the
target it was making, restores the disposition and raises the signal again — so
the process dies of what it was sent and a shell reads 128 + the signal number.
`SIGQUIT` is the one exception, and `commands.c` writes out why: *"We don't want
to send ourselves SIGQUIT, because it will cause a core dump. Just exit
instead."* It exits `MAKE_TROUBLE`, which is 1.

**What Ronin does now.** The same, in both front ends. Measured on 2026-08-24
through `scripts/sandboxed`, the signal sent to the tool's own pid, every tool
exec'd through `perl -e '$SIG{$_}="DEFAULT" for qw(INT QUIT HUP TERM); exec
@ARGV'` — without that reset an inherited `SIG_IGN` for `SIGINT`/`SIGQUIT` makes
GNU Make ignore the signal and the cell measures nothing:

| signal | GNU Make 4.4.1 | Ronin **before** | Ronin **after** |
| --- | --- | --- | --- |
| SIGINT | dies of signal 2 → 130 | exit 130 | **dies of signal 2 → 130** |
| SIGTERM | dies of signal 15 → **143** | exit 130 | **dies of signal 15 → 143** |
| SIGHUP | dies of signal 1 → **129** | exit 130 | **dies of signal 1 → 129** |
| SIGQUIT | exit **1**, no core | exit 130 | **exit 1, no core** |

Twelve Make-mode cells were measured — the four signals × mid-recipe, during the
read phase (a `$(shell)` running), and under `-q` with a `+`-marked line — and
all twelve now agree with GNU exactly. Four Ninja-mode cells (mid-recipe) follow
the same rule, which is the ruled divergence from upstream Ninja.

Note the SIGQUIT row, which the survey had not measured before: **GNU's rule is
not "128 + signum"**. It is "die of the signal, except the one whose default
action dumps core". Following the GNU case means following it there too, so
Ronin leaves 1 for `SIGQUIT` rather than 131 — and writes no core file.

**The SIGQUIT carve-out is confirmed, not inferred.** It was the one fact in the
delivery the operator's stated facts did not include, so it was put back to him
in as many words: does following the GNU case mean 1 for `SIGQUIT` as well?
Operator decision, Brendan, 2026-08-24, verbatim: *"exit 1."* What shipped is
what he confirmed; no code changed on the strength of the confirmation, and this
paragraph is the record of it.

**What did not change.** Everything the interrupt work established is intact and
still gated: the cut-short edge is not recorded in the build log, partial outputs
are removed, the target is withdrawn, `.PRECIOUS` is spared, an interrupted read
abandons its `$(shell)` child rather than waiting for it, and `-q` still refuses
to answer the affirmative zero. A recipe that exits 130 of its own accord is
still an ordinary failure reported as 2, and a recipe killed by a signal nobody
sent the build is still an ordinary failure — neither turns into a signal death,
because the ending is chosen from the signal this process actually caught rather
than from the number the build reported.

**Upstream Ninja, for the record.** `ExitInterrupted` is hard-coded 130
(`reference/ninja/src/exit_status.h:30`) and returned at
`reference/ninja/src/ninja.cc:1679`; `subprocess-posix.cc` installs handlers for
`SIGINT`, `SIGTERM` and `SIGHUP` only and never re-raises any of them. Ronin's
manifest front end now leaves 143 where upstream leaves 130, and handles
`SIGQUIT` where upstream does not handle it at all. That is the divergence the
operator ruled for.

**Where it is written down.** `[spec:ronin:req:product.build-outcome]` said the
opposite verbatim — *"an interrupt leaves with Ninja's 130 rather than
re-raising the signal, so the status does not depend on how far the build had
got; C samurai re-raised here, and Ninja is the contract."* The rule is now
**`+1`** and states the ruling, with all 30 source references re-read and
re-pinned.

**Authority: operator decision, Brendan, 2026-08-24**, quoted in full above.

### 6. `-n` does not run a `+`-marked or `$(MAKE)`-referencing recipe line

**Owner:** `make-recipe-dry-run` (Done). **Gated by:** the discovery-only cases
`tests/make/dry-run-skips-a-plus-line` and
`tests/make/dry-run-skips-a-make-reference-line` (in `DISCOVERY_ONLY_CASES`).

**What GNU does / what Ronin does.** GNU runs a `+`-marked line, and a line whose
unexpanded text names `$(MAKE)`, even under `-n` — because starting the child is
the only way GNU can learn what the child would do. Ronin compiled the child
into the graph, so its `-n` is Ninja's: print the commands, run none of them.
Measured under `-n`:

| makefile | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| `all:` / `+echo plus > plus` | writes `plus` | writes nothing |
| a line running `$(MAKE)`-named text | runs it (writes its file) | writes nothing |

The recursive-`$(MAKE)` case where the child is a real sub-make agrees (Ronin
walks the composed child's edges), which is why this is only ever about a
`+`/`$(MAKE)` line that is *not* a sub-make.

**Reachability.** Only under `-n` (or `-n -t`) with such a line. A real build
without `-n` runs everything in both tools.

**Cost to match.** Would require an executor-side Make exception that launches a
child during `-n` — which is exactly the `DRY_RUN_COMMAND` the compiler-boundary
removal deleted.

**Authority: operator decision, Brendan, 2026-08-24, on this section:**
*"sounds like a bug in GNU, fuck GNU."* Ronin's behaviour is **ratified** and
kept: under `-n` nothing runs, `+` and `$(MAKE)` included. The two
`DISCOVERY_ONLY_CASES` recordings stay where they are — they hold what GNU does,
so the difference is on the record rather than forgotten.

The dispatch decision that preceded him is not overtaken, because it is the
mechanism's rationale rather than the authority; his ruling is now the authority.
`make-recipe-dry-run`, 2026-08-08: *"Dry-run spellings are interface controls
mapped onto the ordinary Ninja dry-run path. Plus-prefixed recursive Make lines
are compiler inputs for subninja composition, not an exception that launches a
child Make during dry-run."* The completion note (2026-08-09) adds: *"The `+`
prefix on a line that is not Make loses its GNU effect, and that is the decision
rather than an oversight."* What his ruling adds is the judgement the dispatch
could not make: running a command under the switch that means *run nothing* is
GNU's bug, not Ronin's gap.

### 7. An output's directory is created where GNU Make would leave it to the recipe — **FIXED 2026-08-24**

**Ruled closed.** Operator decision, Brendan, 2026-08-24, verbatim: *"Make's rule
applies when run as make."* That reverses the 2026-08-19 retirement, which had
recorded the divergence as intentional on the dispatch's own authority. **Make
mode no longer creates an output's directory; Ninja mode still does, unchanged.**

**Owner:** `make-an-output-directory-is-created-where-gnu-make-would-refuse` was
retired (Done, 2026-08-19) as a recorded intentional divergence and is superseded
by `an-outputs-directory-is-left-to-the-recipe-in-make-mode`, which carries the
ruling and the delivery. **Gated by:** four oracle-recorded corpus fixtures —
`an-outputs-directory-is-the-recipes-to-arrange`,
`an-output-in-a-directory-nothing-creates-is-refused`,
`a-touched-output-in-a-directory-nothing-creates-is-refused`,
`a-recipes-own-mkdir-of-its-output-directory-runs` — plus the kati unit pair
`a_redundant_output_mkdir_is_absorbed_for_ninja` /
`a_recipes_own_output_mkdir_is_kept_where_it_owns_the_directory`.

**What GNU does.** `$@`'s directory is the recipe's problem, which is why every
makefile in the world writes `@mkdir -p $(@D)` and why automake generates it.
Ninja creates it, because a manifest is generated and its generator is entitled
to assume the directory is there.

**Measured, before and after.** Four shapes, all through `scripts/sandboxed`
against `reference/make-oracle/make-4.4.1/make` on 2026-08-24. The survey had
recorded only the first; three of the four diverged, and two of those diverged
*silently*, with Ronin succeeding where GNU fails.

| shape | GNU Make 4.4.1 | Ronin **before** | Ronin **after** |
| --- | --- | --- | --- |
| a recipe replaces a file with a directory of the same name | `GEN`, `X=1`, exit 0 | `build stopped: File exists.` + `Not a directory`, **exit 2** | `GEN`, `X=1`, **exit 0** |
| output in a directory nothing creates | exit 2, `sub/out` absent | **exit 0, `sub/out` written** | exit 2, `sub/out` absent |
| recipe makes its own directory | exit 0, `sub/out` written | exit 0, `sub/out` written | exit 0, `sub/out` written |
| `-t` over an output in a directory nothing creates | exit 2, `sub/out` absent | **exit 0, `sub/out` written** | exit 2, `sub/out` absent |

The first shape is the survey's reproducer, unchanged:

```
all: ; @echo "all X=$(X)"
afile/one.mk: ; @echo GEN; rm -f afile; mkdir afile; echo X=1 > afile/one.mk
include afile/one.mk
```

**How, without Make provenance in the graph.** The 2026-08-19 retirement was
right that a per-edge property is forbidden — `plan/decisions/make-compiles-to-ninja.md`
and `docs/make-compiler-boundary-audit.md` rule out an edge property that exists
only to make the executor behave differently because the graph came from a
Makefile. Nothing was written into the graph. `BuildOptions::create_output_directories`
is a **run-level** setting beside the ones the Make front end already answers
(`command_status_interrupts`, `recipe_signal_fails`, `archive_members`, `touch`,
`always_make`): what creates the directory is the launcher, so which launcher
behaviour a run wants is the front end's to say once. A graph loaded from a
manifest still gets Ninja's launcher, and a manifest Make mode emits still
describes a Ninja build.

**The half that had to move with it.** kati absorbs a leading, silent
`mkdir -p $(@D)` from a recipe — sound while Ninja is going to make that
directory anyway, and a deleted build step once nothing does.
`Flags::recipes_own_output_directories` turns the absorption off, and Ronin's
Make front end sets it. This was caught by two `tests/cli.rs` cases
(`recursive_evaluation_waits_for_parent_inputs`,
`nested_recursive_evaluation_boundary`) whose makefiles write `@mkdir -p installed`
and whose recipes had been running without it all along, invisibly, because the
launcher got there first.

**The other thing it was masking.** Exactly one row of
`tests/make_upstream_inventory.tsv` moved, from `unclassified` to `narration`:
`misc/general4.diff.4`, the GNU-suite case named "Make sure that subdirectories
built as prerequisites are actually handled properly... this time with `$`". The
driver runs every test in a script in ONE work directory, and the first test in
that script names the same `dir/subdir`; under the old behaviour Ronin's launcher
created it while running that first test, so by the time the fifth test ran the
directory existed, its `@echo mkdir -p` target was up to date, and one of the
three expected lines never ran. Nothing creates it now, and the case's remaining
difference is the `[N/M]` progress lines alone. Counts: 1285 cases before and
after, unclassified 58 → 57, **compiler 0 → 0**, interface 28 → 28, narration
1199 → 1200. Run that makefile on its own with `dir/` cleared first and both
tools agree either way, which is why it needed the suite's own carried state to
reproduce.

**What Ronin's own staging still does.** A name the compiler invented for itself
— the `.ronin_recipe_stage/N` proxy of a composed recipe's preceding segment —
is not a Make output and never was. Its directory is still made, in
`Builder::prepare_response_file`, because the tool that chose the path is the one
that can place it. `is_virtual_output` keeps the two apart exactly as it did.

**Authority: operator decision, Brendan, 2026-08-24**, quoted in full above. It
is the ruling on directory creation itself that the 2026-08-19 record said did
not exist.

### 8. Recursive keep-going choreography differs (recursive Make is one graph)

**Surfaced by:** `tests/make/makeflags-keep-going-precedence` (in
`DISCOVERY_ONLY_CASES`). **Owner:**
`make-recursive-keep-going-choreography-writes-different-files` (open, filed
2026-08-24). The architecture it runs into is
`make-single-ninja-scheduler` / `make-subninja-recursion` — recursive `$(MAKE)`
invocations compile into one graph with one scheduler, so there is no child Make
runner in which GNU's per-child keep-going choreography could occur. That
explains the divergence; it does not accept it.

**What GNU does / what Ronin does.** A parent whose recipe is
`-@$(MAKE) -S -f stop.mk` then `-@$(MAKE) -k -f go.mk` reaches two separate GNU
processes: the first takes `-S` (stop at the first failure), the second takes
`-k` (carry on to the target beside the failure). Measured:

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| files written | `went` only | `stopped` only |
| exit | 0 (both lines `-@`-ignored) | 2 |

Because Ronin composes both sub-makes into one graph with one scheduler, the
`-S`/`-k` precedence across the composed boundary does not reproduce GNU's
per-process choreography.

**Reachability.** **Yes** — a recursive build that relies on different keep-going
settings per child, under `-k`, can build different files and exit differently.

**Cost to match.** Would require Ronin to preserve a per-child keep-going policy
across composition — i.e. to schedule composed sub-makes as if they were
separate runners, which is the recursive-jobserver model
`make-single-ninja-scheduler` replaced with one scheduler.

**Authority: NOT ratified — the operator's ruling was conditional and the
condition FAILED.** Operator decision, Brendan, 2026-08-24, verbatim: *"if it's
behaviourally equivalent I don't care."* It is not behaviourally equivalent, so
his ruling does not cover it and nothing here is accepted.

**The measurement his condition asked for.** All eight `DISCOVERY_ONLY_CASES`
were run through both tools on 2026-08-24 under `scripts/sandboxed`, mirroring
the `make_port` harness (scratch copy, the case's `setup`, the case's `args`,
then the exit status and every file the run left, by relative path and content):

| case | exit | files | verdict |
| --- | --- | --- | --- |
| `makeflags-keep-going-precedence` | GNU 0 / Ronin **2** | GNU wrote `went`; Ronin wrote `stopped` | **DIFFERS** |
| `makeflags-outranked-by-command-line` | 0 / 0 | identical (8) | same |
| `makeflags-value-switch-precedence` | 0 / 0 | identical (7) | same |
| `makeflags-withdrawal-outranked-by-command-line` | 0 / 0 | identical (6) | same |
| `phony-runs-though-the-file-is-current` | 0 / 0 | identical (4) | same |
| `a-what-if-file-that-is-double-colon-refuses-after-the-chain-ran` | 2 / 2 | identical by content (5) | see §9 — the difference is *when* the file was written, which this probe reads by content and the harness reads by mtime |
| `dry-run-skips-a-plus-line` | 0 / 0 | GNU wrote `plus` | **DIFFERS** — that is §6, open |
| `dry-run-skips-a-make-reference-line` | 0 / 0 | GNU wrote `named` | **DIFFERS** — that is §6, open |

So the keep-going cell differs in **both** halves of the condition: a different
file on disk and a different exit status. The four other `makeflags-*` and
`phony-*` cases are behaviourally identical and differ only in narration, which
is what makes the failing cell a finding rather than a category.

**Filed as a defect:** `make-recursive-keep-going-choreography-writes-different-files`,
with the reproducer above. Do not read the operator's sentence as accepting this.

### 9. `-W` over a double-colon chain refuses before the chain's work runs

**Surfaced by:** `tests/make/a-what-if-file-that-is-double-colon-refuses-after-the-chain-ran`
(in `DISCOVERY_ONLY_CASES`). **Owner:** the architecture that plans the whole
graph before running any of it.

**What GNU does / what Ronin does.** GNU updates a `::` chain entry by entry and
meets a recipe-less entry (always out of date) as it walks, so a `::` rule with
work to do RUNS, and the run is refused only afterwards. Ronin plans the whole
graph before any of it runs, so the refusal comes first and the work never
happens. Measured with `-W out` over a `::` chain:

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| ran `touch out`? | yes — `out` written | no |
| then | `*** No rule to make target 'out'.` exit 2 | `ronin: No rule to make target 'out'.` exit 2 |

Both refuse, both leave the dependent alone; only whether the chain's own work
ran first differs.

**Reachability.** Only under the `-W`/`-t` family over a double-colon chain whose
refusal falls after work — a fringe shape.

**Cost to match.** Would require running part of a graph before the plan is
complete — against Ronin's plan-then-run model, which hands a finished plan to a
frontend and must not hand it a partial one.

**Authority: operator decision, Brendan, 2026-08-24, on this section:** *"fine."*
The architecture it consents to is the plan-then-run model: the whole graph is
planned before any of it runs, so a refusal that GNU Make reaches part-way
through a `::` walk is reached here before the walk starts.

### 10. `-k` builds nothing past an unmakeable prerequisite — **ACCEPTED DIVERGENCE**

**Owner:** `make-keep-going-builds-what-it-can-past-an-unmakeable-prerequisite`,
filed as a defect on 2026-08-24 and **retired the same day as a recorded,
operator-ruled divergence**. Gated by
`tests/make/keep-going-refuses-the-goal-before-building-what-it-can`, so a change
that starts building the three objects reopens the decision rather than passing
silently.

**What GNU does / what Ronin does.** Under `-k`, with one prerequisite that has
no rule at all, GNU Make walks the goal's prerequisites, builds the three it can,
and only then abandons the goal. Ronin refuses at graph-load time — the goal
provably cannot complete, and it says so before running anything.

**Reproducer / measured** (a `work/` directory holding `main.c`, `defs.h`,
`command.h`, `commands.c`, `display.c`, `buffer.h`, `command.c`, and deliberately
no `kbd.c`):

```
VPATH = work
edit:  main.o kbd.o commands.o display.o
	@echo cc -o edit main.o kbd.o commands.o display.o
main.o : main.c defs.h
	@echo cc -c main.c
kbd.o : kbd.c defs.h command.h
	@echo cc -c kbd.c
commands.o : command.c defs.h command.h
	@echo cc -c commands.c
display.o : display.c defs.h buffer.h
	@echo cc -c display.c
```

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| under `-k` | builds `main.o`, `commands.o`, `display.o`, then abandons `edit` | refuses `kbd.c`, builds nothing |
| exit | 2 | 2 |

Same exit status, three fewer files — and none of the three was going to be used,
because the goal that wanted them cannot be made.

**Authority: operator decision, Brendan, 2026-08-24**, verbatim: *"Ronin
superior. Accepted divergence."* Asked whether the node should be kept as a
defect or retired, he answered KEEP the behaviour: fail fast is right, and
refusing at graph-load when the goal provably cannot complete beats doing work
that gets thrown away. So this is the one place in this document where Ronin
deliberately does *less* than GNU Make and the operator ruled that the less is
better.

---

### Two defects this survey found (filed as nodes, not accepted) — **BOTH FIXED**

These were never deliberate divergences; they were bugs, found while surveying
the upstream residue for `make-upstream-residue-triage`, filed as nodes and
since repaired.

**A crash — FIXED 2026-08-25.**
`make-a-define-directive-whose-name-reads-as-an-assignment-panics`. `define = x`
(a `define` directive whose remainder reads as an assignment) made Ronin panic
(`kati/src-rs/parser.rs:1125`, `assertion failed: sep != 0`, exit 101) where GNU
builds cleanly. Nine of the ten spellings the node covers were wrong; all ten
now agree.

**A refused build — FIXED 2026-08-27.**
`make-a-command-line-assignment-with-an-unterminated-reference-is-refused`.
`make 'hello=$(world'` over `all:; $(info good)` — GNU stores the unused value
and builds; Ronin refused with `unterminated variable reference.` because a
compilation unit's exported environment is settled at read time and the failure
was charged there. It is now charged where GNU charges it: at the job that would
have carried the value, which `start_job_command` (job.c) reaches only past the
empty command, `-n` and `-q`. All ten measured cells agree.

**Authority: none was needed — these were defects to fix, not divergences to
accept, and both are fixed.**

---

### What was ruled to be implemented rather than diverge

Two earlier operator rulings, both 2026-08-17, point the *other* way — toward
conformance — and are recorded here for completeness. They predate the rulings of
2026-08-24 that this document now carries on its numbered sections, and neither
was overtaken by them, except where §7 says so below.

**`-t`, `-B`, `-W` are implemented, not accepted no-ops.** Operator decision,
Brendan, 2026-08-17, explicit (on `make-archive-member-touch`): *"implement `-t`
fully. This reverses the accept-without-emulation disposition recorded for
`make-option-touch-and-what-if`."* The reason given: `-t` "decides what the run
writes to disk, and filesystem effects ARE a conformance criterion under
`[spec:ronin:req:make.semantics+1]`." A second ruling the same day scoped it:
*"the BEHAVIOUR lands as specified, the NARRATION does not"* — so a touched edge
is reported by the ordinary `[N/M]` progress line and GNU's `touch <file>`
stdout is not reproduced. `-B` and `-W` followed on the same criterion. So these
switches are **not** divergences: Ronin matches GNU's filesystem effect (the two
oversized/`-n` divergences above are the only residue).

**Make-voiced runtime output is narration, and the runtime speaks Ninja.**
Operator decision, Brendan, 2026-08-17, explicit (on
`make-narration-contract-audit`): *"the runtime speaks Ninja; Make-voiced output
is legitimate ONLY where a failure must be reported at all, and then in Ronin's
established diagnostic shape — never as optional runtime chatter mimicking
GNU."* This is the authority for treating GNU's success-path and progress
chatter — `pattern recipe did not update peer target`, `*** Deleting file`,
jobserver-mode warnings, `touch <file>`, `Entering/Leaving directory`,
up-to-date announcements — as narration Ronin declines rather than as
divergences of build intent. It is why those lines are recognised families in
`tests/make_upstream_inventory.tsv` rather than open divergences.

It was also cited, by analogy, as governing §7 — output-directory creation — on
the grounds that the same boundary decides both. **That analogy no longer
holds**, and it never named the case: on 2026-08-24 the operator ruled directly
on §7 (*"Make's rule applies when run as make."*) and Make mode now leaves the
directory to the recipe. The narration ruling is about what a build SAYS; §7 was
about what it DOES, which is why the two came apart. The boundary itself is
intact — nothing was written into the graph, and the answer is a run-level
setting the front end gives at the launcher.
