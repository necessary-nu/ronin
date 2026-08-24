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
builds of GNU Make — and it is a survey for the operator to rule on rather than
a record of decisions already taken:
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
each.

It is a survey, not a record of settled decisions. The operator's instruction of
2026-08-24, verbatim: *"I do not think we have any accepted divergences. You will
need to explain all the divergences now."* So nothing below should be read as
approved. Where a divergence names an **Authority**, that line says exactly who
recorded it and on what basis — and for most of them the honest answer is that
the delivering dispatch recorded it (authored under the operator's git identity,
but written by the agent), grounded in a spec rule or an architecture decision,
with **no standalone operator ruling**. Two genuine operator rulings do exist and
are cited verbatim where they bear: the narration line and the `-t`/`-B`/`-W`
implementation, both 2026-08-17.

Every measured cell below was re-measured against `reference/make-oracle/make-4.4.1/make`
on 2026-08-24.

### Summary

| # | Divergence | Reachable in a real build? | Gated by | Authority |
| --- | --- | --- | --- | --- |
| 1 | A shuffled order-only prerequisite is permuted apart from the rest | No — only under `--shuffle`, plus one `$|` cell needing a cycle | nothing (open node) | **none** — open, awaiting a ruling |
| 2 | `$(MAKEFLAGS)` hands back the text it stores | Only where a switch value holds a literal `$` | 2 `divergence` sidecars + this doc | dispatch (no operator ruling) |
| 3 | An oversized recipe's marked lines do not run under `-t`/`-q` | No — needs a recipe line over 100 kB | `tests/shell.rs::oversized_marks_are_not_split_out` | dispatch (no operator ruling) |
| 4 | A `.KATI_DEPFILE` recipe's `$(file …)` runs where it is built | No — `.KATI_DEPFILE` is a kati extension no GNU makefile names | `tests/shell.rs::a_depfile_recipe_is_read_where_it_is_built` | dispatch (no operator ruling) |
| 5 | An interrupt leaves Ninja's 130, not GNU's 128+signum | **Yes** — any build killed by SIGTERM/SIGHUP/SIGQUIT | `tests/interrupts.rs`; `product.build-outcome` | spec `product.build-outcome` (no direct operator ruling) |
| 6 | `-n` does not run a `+`-marked or `$(MAKE)`-referencing line | Only under `-n` with such a line | `DISCOVERY_ONLY_CASES` in `make_port` | dispatch decision on `make-recipe-dry-run` |
| 7 | An output's directory is created where GNU leaves it to the recipe | **Yes** — a recipe that replaces a file with a same-named dir | nothing (retired node) | dispatch, citing the boundary + the narration ruling |
| 8 | Recursive keep-going choreography differs | **Yes** — recursive `$(MAKE)` with per-child `-k`/`-S` | `DISCOVERY_ONLY_CASES` in `make_port` | architecture (compile recursive Make to one graph) |
| 9 | `-W` over a `::` chain refuses before the chain's work runs | Only under `-W`/`-t`-family over a double-colon chain | `DISCOVERY_ONLY_CASES` in `make_port` | architecture (whole graph planned before it runs) |
| 10 | Two defects found by this survey | **Yes** (a crash, a refused build) | filed as nodes | **none** — defects, not divergences to accept |

A tenth line worth stating plainly: beyond #10's two, the upstream residue holds
**85 more genuinely unclassified rows** (see `tests/make_upstream_inventory.tsv`
and `make-upstream-residue-triage`) — real build-intent differences left
unclassified on purpose rather than forced into narration. They are a long tail
of individually small divergences (target/pattern-specific variable values,
second-expansion prerequisite order, backslash-newline continuation, include
remaking, `\#` escaping, static-pattern and vpath cases), most already tracked by
open compiler nodes.

---

### 1. A shuffled order-only prerequisite is permuted apart from the rest

**Owner:** `make-shuffle-reorders-order-only-prerequisites-apart-from-the-rest`
(open). Full evidence and the two candidate fix shapes are on that node.

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

**Authority: none.** Open node, no ruling. This survey's line #1 is exactly what
it asks the operator to rule on.

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

**Authority: no operator ruling.** Recorded by the delivering dispatch on
`make-makeflags-holds-its-switches-as-literal-text`, grounded in the argument
that "reproducing a data loss is not compatibility of build intent."

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

**Authority: no operator ruling.** Recorded by the delivering dispatch, grounded
in the reachability evidence above.

### 4. A `.KATI_DEPFILE` recipe's `$(file …)` is performed where it is built

**Owner:** `make-recipe-file-operation-at-launch-only` (Done).
**Gated by:** `tests/shell.rs::a_depfile_recipe_is_read_where_it_is_built` (no
corpus case — `.KATI_DEPFILE` is a kati extension, so GNU Make has no answer to
record against).

**What GNU does / what Ronin does.** GNU expands a recipe only when about to run
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

**Authority: no operator ruling.** Recorded by the delivering dispatch.

### 5. An interrupt leaves Ninja's 130, not GNU's 128 + signal number

**Owner:** `make-mode-leaves-an-interrupt-with-2-where-both-references-say-130`
(Done), with `question-mode-answers-up-to-date-after-an-interrupt` for the `-q`
face. **Gated by:** `tests/interrupts.rs` (e.g. `make_termination_leaves_ninjas_status`).

**What GNU does / what Ronin does.** GNU catches a fatal signal, kills its
children, withdraws the target, and then dies of the signal — so its exit is
128 + the signal number. Ronin leaves Ninja's fixed interrupt code, 130, for
every interrupt, without re-raising. Measured, each tool signalled on itself
mid-recipe, through `scripts/sandboxed`:

| signal | GNU Make 4.4.1 | Ronin | target |
| --- | --- | --- | --- |
| SIGINT | 130 | 130 | deleted, in both |
| SIGTERM | **143** | **130** | deleted, in both |
| SIGHUP | **129** | **130** | deleted, in both |

SIGINT happens to agree on the number; SIGTERM and SIGHUP diverge. The files
agree in every case. (The number also agrees on SIGINT while the *mechanism*
differs — GNU re-raises, Ronin does not — which is the same trade one level up.)

**Reachability.** **Yes** — any real build killed by SIGTERM (or SIGHUP/SIGQUIT)
leaves a different exit code than GNU. This is the one divergence in this survey
that an ordinary, un-fringe build reaches.

**Cost to match.** Re-raising the signal in Make mode and not in Ninja mode —
runtime emulation in the one front end that exists to avoid it. The two modes
share one interrupt path.

**Authority: a spec rule, not a direct operator ruling.**
`[spec:ronin:req:product.build-outcome]` states it verbatim: *"an interrupt
leaves with Ninja's 130 rather than re-raising the signal, so the status does
not depend on how far the build had got; C samurai re-raised here, and Ninja is
the contract."* The Make-mode delivery (2026-08-22) applied that rule; the
divergence from GNU's 143 is stated by the gate `make_termination_leaves_ninjas_status`.
No operator ruling names SIGTERM specifically.

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

**Authority: a dispatch decision, no operator ruling.** `make-recipe-dry-run`,
2026-08-08: *"Dry-run spellings are interface controls mapped onto the ordinary
Ninja dry-run path. Plus-prefixed recursive Make lines are compiler inputs for
subninja composition, not an exception that launches a child Make during
dry-run."* The completion note (2026-08-09) adds: *"The `+` prefix on a line
that is not Make loses its GNU effect, and that is the decision rather than an
oversight."*

### 7. An output's directory is created where GNU Make would leave it to the recipe

**Owner:** `make-an-output-directory-is-created-where-gnu-make-would-refuse`
(retired as a recorded intentional divergence — no code, no gate, and until now
not in this doc). **Gated by:** nothing.

**What GNU does / what Ronin does.** Ninja creates an edge's output directory
before launching the command, and Ronin runs Make's recipes through Ninja's
launcher, so it does too. GNU leaves `$@`'s directory to the recipe. Where the
name is free the two agree; where the name is taken by a non-directory, they
part. Measured, with `afile` an ordinary file:

```
all: ; @echo "all X=$(X)"
afile/one.mk: ; @echo GEN; rm -f afile; mkdir afile; echo X=1 > afile/one.mk
include afile/one.mk
```

| | GNU Make 4.4.1 | Ronin |
| --- | --- | --- |
| result | `GEN`, `all X=1`, exit 0 (the recipe cleared the file, made the dir, read the fragment) | `ronin: build stopped: File exists.` + `Makefile:3: afile/one.mk: Not a directory`, exit 2 |

**Reachability.** **Yes** in principle — any recipe whose first job is to arrange
its own output directory (replacing a file with a directory of the same name is
the shape that reaches it). Nothing in any corpus depends on either answer,
which is why it surfaced from a hand probe rather than a gate.

**Cost to match.** A per-edge property the Make sink sets and the manifest parser
leaves off — which
`plan/decisions/make-compiles-to-ninja.md` and `docs/make-compiler-boundary-audit.md`
forbid: "do not create my output's directory" is not a Ninja graph idiom, and an
edge property that exists only to make the executor behave differently because
the graph came from a Makefile is Make provenance in the graph.

**Authority: a dispatch decision, citing the boundary and an operator ruling by
analogy.** Recorded by the dispatch that retired the node (2026-08-19), grounded
in the compiler-boundary audit and pointing at the narration operator ruling
(below) — but there is **no operator ruling on directory creation itself**. If
reopened it should be a `plan/decisions`-level question about who owns an
output's directory for *every* front end, not a Make special case.

### 8. Recursive keep-going choreography differs (recursive Make is one graph)

**Surfaced by:** `tests/make/makeflags-keep-going-precedence` (in
`DISCOVERY_ONLY_CASES`). **Owner:** the architecture of
`make-single-ninja-scheduler` / `make-subninja-recursion` — recursive `$(MAKE)`
invocations compile into one graph with one scheduler, so there is no child Make
runner in which GNU's per-child keep-going choreography could occur.

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

**Authority: architecture, no operator ruling.** This is a documented consequence
of compiling recursive Make into one graph. The `make_port` harness records it as
discovery-only precisely because "recursive Make invocations compile into one
graph, so there is no child Make runner in which that choreography could occur."

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

**Authority: architecture, no operator ruling.** A consequence of planning the
whole graph before running it.

### 10. Two defects this survey found (filed as nodes, not accepted)

These are not deliberate divergences; they are bugs, found while surveying the
upstream residue for `make-upstream-residue-triage`, and filed as nodes.

**A crash.** `make-a-define-directive-whose-name-reads-as-an-assignment-panics`.
`define = x` (a `define` directive whose remainder reads as an assignment) makes
Ronin panic (`kati/src-rs/parser.rs:1125`, `assertion failed: sep != 0`, exit
101) where GNU builds cleanly. A panic reachable from a plain makefile line.

**A refused build.**
`make-a-command-line-assignment-with-an-unterminated-reference-is-refused`.
`make 'hello=$(world'` over `all:; $(info good)` — GNU stores the unused value
and builds; Ronin refuses with `unterminated variable reference.` because it
reads the command-line assignment's value eagerly.

**Authority: none — these are defects to fix, not divergences to accept.**

---

### What was ruled to be implemented rather than diverge

Two genuine operator rulings exist, both 2026-08-17, and both point the *other*
way — toward conformance — so they are recorded here for completeness, to show
where the line has actually been drawn by the operator rather than by a dispatch.

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
`tests/make_upstream_inventory.tsv` rather than open divergences, and it governs
divergence #7 above by analogy (the same boundary decides output-directory
creation), though it does not name that case.
