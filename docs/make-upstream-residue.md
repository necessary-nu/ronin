# The 85 unclassified upstream rows, explained

`tests/make_upstream_inventory.tsv` records every case in GNU Make 4.4.1's own
test suite whose output differs from Ronin's. On 2026-08-24 it held 1285 rows,
85 of which were `unclassified` — the class that means nothing more than *nobody
has established why this differs*. The operator's instruction that day, verbatim:

> "I do not think we have any accepted divergences. You will need to explain all
> the divergences now."

This document is that explanation. Every one of the 85 rows reached an evidenced
outcome; none was moved into a narration family to make a number smaller, and
none is recorded here as an accepted divergence. Where a row is a real
build-intent difference it has a filed node with a minimal reproducer. Where a
row's residue was not a Ronin divergence at all, the measurement that shows so is
written out.

Every measured cell below was taken on 2026-08-24 against
`reference/make-oracle/make-4.4.1/make` (GNU Make 4.4.1, `Built for
x86_64-pc-linux-gnu`) and against Ronin at `a6d8d19`, each in a fresh directory,
with a `make`-named symlink for Ronin because Make mode is reached by the invoked
name. The suite itself is the pinned checkout at `reference/gnumake`
(`d66a65ad`), read for what each case asserts.

This is a companion to [`make-oracle-divergences.md`](make-oracle-divergences.md),
which surveys the divergences reachable from a real build. That document's
summary named these 85 rows as "a long tail of individually small divergences";
this one replaces that sentence with the actual list.

---

## Summary

| Outcome | Rows | What it means |
| --- | ---: | --- |
| **Defect** | **36** | A real build-intent difference. 22 nodes filed under `make-subsumption`, each with a minimal reproducer. Not fixed here — this was a diagnosis dispatch. |
| **Narration — family added** | **23** | The two tools say the same thing differently. The classifier now names the reason, so these rows left `unclassified`. |
| **Narration — documented only** | **7** | Same finding, but the recognition could not be mechanised safely. These rows stay `unclassified` on purpose; the reason each resisted is stated. |
| **Interface — family added** | **4** | GNU's `MAKEFLAGS` carries `--jobserver-auth=`, which a single-scheduler Ronin has no transport for. Recorded as an interface observation, not narration. |
| **Suite artifact** | **7** | Not a Ronin divergence at all: the residue comes from state the suite's shared `tests/` directory carries between scripts. Measured with the same state, GNU produces Ronin's output exactly. |
| **GNU-internal** | **5** | The case asserts GNU's own implementation — its stdin temp file, its re-exec, its jobserver FIFO fallback, its output-sync temp file — rather than Make semantics Ronin owes. |
| **Duplicate** | **3** | Already owned by a filed node or by a survey entry. |
| **Undiagnosed** | **0** | — |
| **Total** | **85** | |

**Count movement: 85 → 58.** Twenty-seven rows left `unclassified` (23 narration,
4 interface). The other 31 explained rows deliberately stay `unclassified`,
because that class is what keeps a real difference visible: a defect belongs in
it until it is fixed, and a suite artifact or a GNU-internal case has no honest
family to move into. The compiler bucket stayed **0**.

Inventory after: `unclassified 58 / compiler 0 / interface 28 / narration 1199`.

### Families added to `examples/make_upstream.rs`

| Family | Class | What it recognises |
| --- | --- | --- |
| `recursive-invocation-echo` | narration | GNU echoed a recipe line that runs Make again; Ronin composes the child into the same graph and launches no such command. Read from the make program the driver actually invoked, off the case's own `.run` file. Off under `-n`. |
| `waiting-for-jobs` | narration | GNU's `*** Waiting for unfinished jobs....`. Ronin waits for the same recipes and says nothing. |
| `jobserver-auth` | interface | Only `--jobserver-auth=…` is deleted from GNU's line; what is left has to be what Ronin printed. |
| `trace-line` | narration | GNU's `--trace` `update target 'X' due to: …`, only when the run asked for `--trace`. |
| `touch-announce` | narration | GNU's `touch <file>` under `-t`, only when `-t` ran and `-n` did not. |
| `include-remake-refusal` | narration | Both tools refuse a run whose `include` cannot be made; Ronin adds the rule search that failed. |

`ninja-failure-block` was also widened: the block's echoed command line is now read
through whatever launcher the makefile chose — `/usr/bin/perl -c "…"`,
`/bin/sh -ec "…"`, `cd 'dir' && exec env … ` — instead of only `/bin/sh -c "…"`,
and only in a case whose output holds a `FAILED:` for the block to belong to.

Two candidate families were tried and **withdrawn** for taking rows they should
not have:

* Reading Ronin's own include diagnostics line by line broke the `shared-refusal`
  cancellation on `features/include.diff.22`, `.23` and `options/dash-I.diff`,
  `.4`, `.8`, and pushed `features/include.diff.17` into the compiler bucket. The
  recognition is now a property of the whole case instead.
* Letting `touch-announce` fire under `-n` would have swallowed
  `options/dash-n.diff.4`, where the two tools report *different intended work*.
  It is gated off under `-n` for that reason.

### Nodes filed

All under `make-subsumption`, all `kind fix`, all carrying the reproducer, GNU's
measured answer, Ronin's, and the mechanism where it was found.

| Node | Rows |
| --- | --- |
| `make-an-escaped-hash-in-a-function-argument-keeps-its-backslash` | features/escape.8 |
| `make-a-normal-prerequisite-outranks-the-same-name-as-order-only` | features/order_only.3, .4 |
| `make-a-foreach-variable-name-is-trimmed-of-whitespace` | functions/foreach.2, misc/bs-nl.20–.25 |
| `make-a-shell-assignment-keeps-a-hash-its-command-printed` | features/shell_assignment.1 |
| `make-second-expansion-sees-target-and-pattern-specific-variables` | features/se_explicit.3, se_implicit.1, se_statpat.1 |
| `make-second-expanded-prerequisites-keep-their-rule-line-order` | features/se_explicit.20 |
| `make-an-existing-target-is-not-rebuilt-through-a-declined-implicit-chain` | features/se_implicit.13, .18 |
| `make-a-library-prerequisite-is-the-file-libpatterns-names` | features/se_explicit.10 |
| `make-an-empty-static-pattern-prerequisite-is-not-an-empty-path` | features/statipattrules.3 |
| `make-a-grouped-pattern-target-keeps-its-own-explicit-prerequisites` | features/patternrules.7 |
| `make-every-matching-pattern-specific-assignment-composes-in-order` | features/patspecific_vars.3 |
| `make-a-command-line-variable-outranks-a-target-specific-assignment` | features/targetvars.1 |
| `make-a-target-specific-export-flag-reaches-the-prerequisites` | features/targetvars.39, .43 |
| `make-a-vpath-resolved-name-keeps-its-explicit-rule` | features/vpath.4 |
| `make-a-byte-order-mark-is-not-part-of-the-first-target` | features/utf8 |
| `make-a-makefile-written-during-evaluation-is-there-to-include` | features/include.27 |
| `make-an-eval-only-invocation-needs-no-makefile-on-disk` | features/include.36 |
| `make-makeoverrides-reaches-a-recursive-invocation` | features/recursion.2 |
| `make-a-command-line-variable-reaches-a-shell-functions-recursive-make` | variables/MAKEFLAGS.116, .117 |
| `make-a-link-rule-does-not-inherit-the-compile-rules-source` | misc/general4.1 |
| `make-touch-touches-a-target-whose-recipe-expands-to-nothing` | options/dash-n.4 |
| `make-a-warning-gnu-emits-about-a-makefile-is-emitted` | options/warn-undefined-variables.2, .3; variables/define.7 |
| `make-keep-going-builds-what-it-can-past-an-unmakeable-prerequisite` | *(none — found in passing, see options/dash-k below)* |

---

## The suite's shared working directory

Seven rows turned out not to be Ronin divergences. They deserve stating first,
because the mechanism is the same each time and it is a property of the harness
rather than of either tool.

The suite runs every script in one `tests/` directory, and a `.base` file holds
a literal string the script's author measured on GNU Make in a *clean*
directory. Scripts leave files behind. By the time a later script runs, the
directory can hold a target's name — `features/output-sync` leaves `bar/` and
`foo/` behind as directories; `features/vpath` leaves `work/kbd.c`;
`misc/general4` leaves `dir/subdir` — and both tools then see a target that
already exists and skip it. The `.base` does not, because it was written before
any of that.

**Corrected 2026-08-27: not all of that state is the harness's.** Measured with
both tools in matched disposable worktrees, GNU Make's own run leaves nothing in
`tests/` beside the driver's own `work/` directory — which is where
`features/vpath`'s `kbd.c` lives, and that one really is the harness's. Two of
the leftovers named above are Ronin's:

* `features/output-sync` leaves `bar/` **only under Ronin**, and not as a race.
  The script's `output_sync_clean` removes the files it named and then `rmdir`s
  the directory; the `rmdir` fails because Ronin's `.ninja_log` and
  `.ninja_deps` are inside it. `foo/` goes because its sub-make was lifted into
  the parent's graph, and `bar/`'s was not — the recipe line has a `;` in front
  of the invocation, so a real child process runs there and opens its own
  persistence. Recorded on
  `make-a-run-leaves-no-directory-the-suite-did-not-ask-for`; the directory half
  is product scope, since Ronin keeps build state where it builds and GNU Make
  keeps none.
* `features/patternrules` left `a.15` and `a.2` **only under Ronin**, and that
  one was a defect: disposability was decided per edge where GNU Make decides it
  per file, so a multi-target pattern rule's outputs all took the answer of
  whichever name the walk reached first. Fixed under
  `make-a-shared-rules-outputs-are-each-swept-on-their-own-answer`; the script
  now leaves nothing but Ronin's own logs.

The rows in the table below still stand as written — recreate the case with the
state the suite had and GNU produces Ronin's output exactly — but the sentence
above them should not be read as saying that every leftover is the harness's.

The test for this is direct: recreate the case *with the state the suite had*
and run both tools. In all seven, GNU then produces Ronin's output exactly.

| Row | The state | Measured |
| --- | --- | --- |
| `features/patspecific_vars.diff.5` | `bar/` exists (from output-sync) | With `bar/` present GNU prints only `pattern: …` — the same one line Ronin printed. Without it both print both lines. |
| `features/patspecific_vars.diff.6` | same, `rec=1` | Same, both modes. Verified with the suite's own `patspecific_vars.mk.1` verbatim. |
| `misc/general4.diff.4` | `dir/subdir` exists | With it present GNU skips `mkdir -p dir/subdir` exactly as Ronin did; without it both run all three lines. |
| `options/dash-k.diff` | `work/kbd.c` exists (from features/vpath) | The makefile's `VPATH` points at `work/`. With `kbd.c` there, GNU builds all five objects, as Ronin did. The `.base` expects `No rule to make target 'kbd.c'`, which is the clean-directory answer. |
| `targets/DEFAULT.diff` | `bar/` exists | Goal `bar`; with `bar/` present both say up to date. With it absent both run the `.DEFAULT` recipe and print `Executing rule BAR`. |
| `variables/automatic.diff.7` | `foo/` and `bar/` exist | With both present GNU says `'foo' is up to date.` and Ronin `no work to do.` — narration. With `foo` absent both print `$? = bar`. |
| `variables/private.diff.7` | `bar/` exists | With `bar/` present GNU prints four lines, not five — the same four Ronin printed. |

These rows stay `unclassified`. The classifier reads a diff and a makefile; it
cannot see the directory the case ran in, so there is no honest family for them.

**One real finding came out of this section.** The `options/dash-k` reproducer,
built to test the artifact theory and run *without* `kbd.c`, exposed a genuine
`-k` divergence: GNU builds the three objects it can and only then abandons
`edit`, while Ronin refuses at graph-load time and builds nothing. Three files
GNU writes and Ronin does not. Filed as
`make-keep-going-builds-what-it-can-past-an-unmakeable-prerequisite`. It is not
what row `options/dash-k.diff` measures, so the row's own outcome is still
*suite artifact*.

---

## features/output-sync — 7 rows

The category exercises `-O`/`--output-sync` over recursive `$(MAKE) -C` builds.

* **`.diff`, `.1`, `.2`, `.4` — narration (`recursive-invocation-echo`).** GNU
  echoes the recipe line `…/make -C foo` because it launches a child process.
  Ronin compiles the recursive invocation into the same graph
  (`make-subninja-recursion`, `make-single-ninja-scheduler`), so no `make`
  process is ever launched and there is no command to echo. Everything the
  sub-build itself prints — `foo: start`, `bar: end`, `baz: …` — appears on both
  sides; with the echo accounted for, the two residues are the same lines, and
  each of these runs is `-j`, so their order is the scheduler's answer rather
  than either tool's.
* **`.3` — narration (`recursive-invocation-echo` + `ninja-failure-block`).**
  Same, plus Ronin's failure block, whose echoed command line here is
  `cd '…/foo' && exec env … /bin/sh -c "…"`. The block recogniser had never been
  taught the `cd '…' && ` a recursive sub-unit grows; it has been now.
* **`.15` — GNU-internal.** The script (output-sync:381) creates a
  mode-`0500` `TMPDIR` and asserts GNU says `/suppressing output-sync/`. That is
  GNU reporting that *it could not create its own temporary file* and is
  carrying on without output synchronisation. Ronin has no output-sync temp
  file; it is one process with one scheduler and one output stream. Measured on
  the makefile itself (`all:; $(info hello, world)` under `-Orecurse`), both
  tools print `hello, world`; GNU adds `'all' is up to date.` and nothing else.
  Not a contract Ronin holds.
* **`.16` — narration, documented only.** The very next line of the same script
  reruns the same makefile and asserts `/#MAKE#: 'all' is up to date./`.
  Measured: GNU prints `hello, world` then `make: 'all' is up to date.`; Ronin
  prints `hello, world` then `ronin: no work to do.` That is exactly the
  existing `up-to-date-line` family. It is not recognised here because the
  suite wrote the expectation as a bare regex rather than as text, so the
  classifier sees `/make: 'all' is up to date./` — slashes and all — as an
  unrecognised line. Unwrapping bare regexes generally is not safe (regex
  metacharacters would be compared as literals), so this row is left explained
  but unmoved.

## features/include — 7 rows

* **`.20`, `.21` — narration (`include-remake-refusal`).** `include hello.mk`
  with a rule to remake it that is either double-colon (`.20`) or `.PHONY`
  (`.21`). Measured: **both tools refuse**, both exit 2, neither builds
  anything, and both print `Makefile:3: hello.mk: No such file or directory`.
  The whole of the difference is Ronin's extra sentence `No rule to make target
  'hello.mk'.` GNU declines to use either rule shape for makefile remaking and
  says so by saying nothing more.
* **`.32`, `.33`, `.35` — narration (`include-remake-refusal`).** A pattern rule
  `%_a.mk %_b.mk:; exit 1` used to remake included makefiles. Measured: both
  tools run `exit 1`, both fail, both exit 2, neither builds. GNU summarises
  with `Failed to remake makefile 'inc_b.mk'.`; Ronin reports the same failure
  as the include's own `inc_b.mk: No such file or directory` plus Ninja's
  failure block. `.33` is `.32` under `-k` and `.35` is the mirrored makefile;
  all three measure the same way.
* **`.27` — DEFECT** (`make-a-makefile-written-during-evaluation-is-there-to-include`).

      default:; @echo $(hello)
      -include hello.mk
      $(shell echo hello=world >hello.mk)
      include hello.mk

  GNU prints `world` and exits 0. Ronin prints `Makefile:2: hello.mk: No such
  file or directory` and `No rule to make target 'hello.mk'.`, and exits 2. The
  `$(shell)` does run — `hello.mk` is on disk after both runs — but the
  following `include` does not see it, and the diagnostic is attributed to the
  `-include` on line 2, which GNU keeps silent.
* **`.36` — DEFECT** (`make-an-eval-only-invocation-needs-no-makefile-on-disk`).
  `make -E 'all:;@echo hi'` in an empty directory: GNU prints `hi`; Ronin says
  `no targets specified and no makefile found` and exits 2. With any makefile
  present the same `-E` works on both, so `--eval` is read — it just does not
  count as makefile content. The suite's own case goes further: with
  `MAKEFILES='foobar barfoo'` and an eval'd `%:;@echo $@`, GNU's makefile search
  itself runs the pattern rule and echoes bizbaz, bazbiz, foobar, barfoo,
  GNUmakefile, makefile, Makefile.

## variables/MAKEFLAGS + variables/GNUMAKEFLAGS — 7 rows

* **`MAKEFLAGS.diff.1`, `MAKEFLAGS.diff.97` — narration (`trace-line`).** Run
  with `--trace`. The only residue is GNU's per-target trace line,
  `work/variables/MAKEFLAGS.mk.1:2: update target 'all' due to: target does not
  exist`. Ronin's counterpart is Ninja's progress counter, which names the
  target without the reason. The `MAKEFLAGS` value itself agrees exactly on both
  sides (`erR --trace --no-print-directory`).
* **`GNUMAKEFLAGS.diff.1` — narration, documented only.** The identical trace
  line, and identical agreement on the `MAKEFLAGS` value. It is not recognised
  because this case gets `--trace` from the `GNUMAKEFLAGS` *environment
  variable*, and the suite's `.run` file records only the command line — so the
  classifier has no evidence that a trace was asked for, and the family is
  deliberately gated on that evidence.
* **`.116`, `.117` — DEFECT**
  (`make-a-command-line-variable-reaches-a-shell-functions-recursive-make`).
  `v:=$(shell $(MAKE) -C bye --no-print-directory)` (and the `!=` spelling) with
  `hello=world` on the command line, against a `bye/makefile` that sets
  `hello=moon`. GNU captures `world`; Ronin captures `moon`. GNU's sv 63347
  regression: a command-line definition must outrank a submake's own.
* **`.119` — DUPLICATE.** `make 'hello=$(world'` over `all:; $(info good)`. GNU
  builds; Ronin refuses with `unterminated variable reference.` Owned by
  `make-a-command-line-assignment-with-an-unterminated-reference-is-refused`
  (open); the row id is now recorded on that node.
* **`.132` — narration (`recursive-invocation-echo`).** `all:; $(MAKE) -C lib2`.
  GNU echoes the recursive line and brackets the child with Entering/Leaving;
  Ronin composes `lib2` into the same graph. Everything else about the two runs
  already cancelled.

## misc/bs-nl + functions/foreach — 7 rows

`misc/bs-nl.diff.20` through `.25` and `functions/foreach.diff.2` are **one
defect** counted seven ways, filed as
`make-a-foreach-variable-name-is-trimmed-of-whitespace`.

Each case writes `$(foreach <name>, <list>, <body>)` with whitespace around the
*name* — from a backslash-newline continuation, a carriage return, a tab, or all
three, once inside a `define` body and once inside a recipe. Measured on the
minimal form:

    $(foreach \
      a \
    , b c d \
    , $(info [$a]))
    all:;@:

GNU prints `[b] [c] [d]`; Ronin prints `[] [] []`. The list argument splits
identically on both sides — Ronin runs the body the right number of times — so
the whole fault is in the name: GNU strips whitespace from it before binding
(`func_foreach`, function.c), and Ronin binds a name that still carries it, so
`$a` finds nothing.

## targets/ONESHELL — 5 rows

* **`.8`, `.9` — narration (`ninja-failure-block`).** `SHELL = /usr/bin/perl`
  with and without an empty `.SHELLFLAGS`. Both tools fail with perl's own
  `Can't open perl script "print "it works"": No such file or directory`; GNU
  adds `*** [ONESHELL.mk.8:5: all] Error 2` and Ronin adds Ninja's failure
  block. The block's echoed command line is `exec env … /usr/bin/perl … "print
  \"it works\""`, which the recogniser did not read because it only knew
  `/bin/sh -c "`. Widened.
* **`.3`, `.4`, `.5` — narration, documented only.** Under `.ONESHELL:` the
  whole recipe is one command. GNU applies the leading `@` of the first line to
  the entire script and prints nothing (`.3`, `.4`) or only the script's output
  (`.5`). Ronin prints Ninja's progress line, whose payload is that same
  multi-line command, so its continuation lines land in the diff as unmatched:

      ronin | [ 0"$a" -eq "$$" ] || echo fail            (.3, .4)
      ronin | +7;  @y=qw(a b c);  print "a = $a, …"      (.5)

  Both tools run the identical script and produce identical output otherwise —
  `.5`'s `a = 12, y = (a b c)` is on both sides. This is `ninja-progress` with a
  command that happens to contain newlines. It is not mechanised because
  recognising a continuation line requires matching it back to the makefile's
  own recipe text, and the two obstacles are real: the recipe lines here carry
  leading whitespace that the echoed text does not, and `.5` sets
  `.RECIPEPREFIX = >` so the classifier's tab-based recipe reader sees no recipe
  at all. Absorbing them on a looser rule would risk deleting a recipe's actual
  output, which is the failure mode the family discipline exists to prevent.

## features/jobserver — 5 rows

* **`.diff`, `.1`, `.2`, `.3` — interface (`jobserver-auth`).** Each makefile
  prints `$(MAKEFLAGS)` in a parent and in a recursive child. The values agree
  in every respect but one: GNU's carries `--jobserver-auth=<auth>` and Ronin's
  does not. `make-single-ninja-scheduler` composes recursive Make into one graph
  with one scheduler, so there is no jobserver and no handle to publish. `.2`
  and `.3` additionally carry GNU's `-jN forced in submake: resetting jobserver
  mode` warning, which the existing `jobserver-narration` family already covers.
  The new family deletes *only* that one switch: if what is left is not what
  Ronin printed, the case stays unexplained.
* **`.13` — GNU-internal.** The script (jobserver:174) sets `TMPDIR=nosuchdir`
  to force GNU's `mkfifo` to fail, and asserts the resulting `No such file or
  directory`; the very next case asserts that make then succeeds anyway. It is a
  regression test for GNU's own FIFO-to-pipe fallback (sv 62908). Measured on
  the makefile (`all:` under `-j2`): GNU says `Nothing to be done for 'all'.`,
  Ronin `no work to do.` — the second half of the test, and ordinary
  `no-work-line` narration. Ronin has no jobserver FIFO to fail to create.

### Update, 2026-08-31: five of these rows are gone

The reading above — "there is no jobserver and no handle to publish" — was the
defect rather than the explanation. `.FEATURES` claimed `jobserver` and
`jobserver-fifo` the whole time, and a `$(MAKE)` Ronin could not compose forked
a real Make that ran `-j` of its own beside its parent's.
`[spec:ronin:req:make.jobserver+3]` makes the claim true: one budget, published
where GNU publishes it, joined and republished the way GNU joins and
republishes it. The four `jobserver-auth` rows lost their only difference and
are now `narration` — Ninja's progress line and the recipe echo, nothing else.

`.15` (`MAKE_TMPDIR=.` beside `TMPDIR=nosuchdir`, asserting
`/--jobserver-auth=fifo:\./`) went further and left the inventory: Ronin reads
`MAKE_TMPDIR` before `TMPDIR` as GNU's `get_tmpdir` does, so the fifo lands in
`.` and the case passes outright.

`.13` is unchanged and stays where it was. Ronin has the same forgiveness GNU's
fallback provides, by a shorter route — a `TMPDIR` that is not a directory is
passed over for `/tmp`, and a fifo it still could not create costs the budget
rather than the build — and says nothing while doing it. GNU's two lines about
it joined `jobserver-narration`.

The `jobserver-auth` family stays, narrowed to the runs where GNU stands a
jobserver up and Ronin does not. Both tools' addresses are anonymised to
`<auth>` before anything is compared, because the value names a temporary file
minted per process and there is nothing byte-comparable in it — that the
address Ronin publishes is one a child can actually spend is gated by peak
concurrency in `an_unlifted_recursion_draws_on_the_shared_budget`, not by
bytes.

## features/temp_stdin — 4 rows

* **`.diff` — narration, documented only.** `make -f temp_stdin.mk -v -f-`. The
  suite asserts `/uilt for /`, matching GNU's `Built for x86_64-pc-linux-gnu`
  version banner. Ronin prints its own: `GNU Make compatible: ronin 0.1.0` /
  `Make front end for GNU Make 4.4.1 makefiles`. This is the product's own
  identity, the same decision as the `product-name` family, and it is
  deliberate. Not mechanised: a version banner has no shape to recognise beyond
  "the text the `-v` switch prints", and gating a family on `-v` for one row is
  not worth the surface.
* **`.4`, `.5`, `.6` — GNU-internal.** All three test GNU's mechanism for
  reading a makefile from standard input: GNU copies stdin to a temporary file
  so that it can *re-exec itself* after remaking makefiles, and passes the copy
  back to itself with the private switch `--temp-stdin=`.
  * `.4` asserts the re-exec command line contains
    `Re-executing.+?--temp-stdin=…/_tmp`.
  * `.5` has the makefile-remaking recipe `chmod u-x` the make binary and
    asserts GNU then fails to re-exec with `Permission denied`.
  * `.6` makes the temp directory unwritable and asserts
    `cannot store makefile from stdin to a temporary file.  Stop.`

  Ronin compiles once and never re-executes, so it has no stdin temp file, no
  `--temp-stdin` switch and no re-exec to fail. Measured on `.4`'s makefile,
  Ronin performs the makefile remake the case is built around (`touch bye.mk`)
  and then evaluates `$(info hello)`, which is the *effect* GNU's restart
  exists to achieve. The switch and the restart are GNU's implementation of it.

## features/se_explicit, se_implicit, se_statpat — 7 rows

Every one is a defect; they group into four causes.

* **Second expansion runs in the wrong scope** — `se_explicit.3`,
  `se_implicit.1`, `se_statpat.1`, filed as
  `make-second-expansion-sees-target-and-pattern-specific-variables`.
  GNU expands a deferred prerequisite with the target's own variable set
  installed. Ronin expands it with only the global set, so the prerequisites
  come back empty.

      .SECONDEXPANSION:
      .DEFAULT: ; @echo 'default $@'
      foo.x: $$a $$b
      foo.x: a := bar
      %.x: b := baz

  GNU builds `bar` and `baz`; Ronin says `no work to do.` `se_implicit.1` gets
  the target-specific half right and the pattern-specific half wrong, which
  points at where the scope is assembled.
* **Prerequisite order across rule lines** — `se_explicit.20`, filed as
  `make-second-expanded-prerequisites-keep-their-rule-line-order`. Two rule
  lines for `hello.tsk`, one recipe-less. GNU's `$^` is `hello.h hello.o`;
  Ronin's is `hello.o hello.h`. Build order differs and so does the recipe's
  command line.
* **An implicit chain GNU declines** — `se_implicit.13`, `.18`, filed as
  `make-an-existing-target-is-not-rebuilt-through-a-declined-implicit-chain`.
  Both need the suite's leftover files to reproduce, and both do reproduce
  exactly with them: where the goal exists and the chain's intermediate does
  not, GNU says `Nothing to be done` and Ronin rebuilds. With every file present
  or none present the two agree.
* **`-lNAME` is not resolved through `.LIBPATTERNS`** — `se_explicit.10`, filed
  as `make-a-library-prerequisite-is-the-file-libpatterns-names`. GNU resolves
  `-lcat` to `libcat.a` and treats them as one file node — its three-line
  warning is precisely the report of that merge — so it runs one recipe. Ronin
  keeps `-lcat` as its own node and runs three. The file written is the same;
  the graph is not.

## features/patspecific_vars — 3 rows

* **`.3` — DEFECT** (`make-every-matching-pattern-specific-assignment-composes-in-order`).
  The suite's TEST #4, "multiple patterns matching the same target":

      a%: AAA = aaa
      %b: BBB = ccc
      a%: BBB += ddd
      %b: AAA ?= xxx
      %b: AAA += bbb
      ab: ; @echo $(AAA); echo $(BBB)

  GNU `aaa bbb` / `ccc ddd`; Ronin `xxx bbb` / `ccc`. Two independent losses: a
  `?=` from a second pattern overrides a value a first pattern already set, and
  a `+=` from a second pattern is dropped entirely.
* **`.5`, `.6` — suite artifact.** See the shared-directory section; measured
  identical with `bar/` present, both with and without `rec=1`.

## features/targetvars — 3 rows

* **`.1` — DEFECT** (`make-a-command-line-variable-outranks-a-target-specific-assignment`).
  `make one two FOO=1 BAR=2` with `one: override FOO = one` and `two: BAR =
  two`. Both agree on `one`, where the assignment says `override`. On `two`, GNU
  keeps the command line's `2` and Ronin lets the plain target-specific `two`
  win.
* **`.39`, `.43` — DEFECT** (`make-a-target-specific-export-flag-reaches-the-prerequisites`).
  `mid: export hello=mid` / `mid: unexport hello=mid` over a `base` prerequisite
  with its own `hello=base`. Both tools agree about `mid` and about `all` and
  disagree about `base`: GNU inherits the export decision down the prerequisite
  chain and Ronin inherits only the value. It is wrong in both directions —
  `export` fails to reach `base`, and `unexport` fails to stop `base` being
  exported.

## targets/POSIX — 3 rows

**`.diff`, `.4`, `.5` — narration (`ninja-failure-block`).** `.POSIX:` sets
`.SHELLFLAGS = -ec`, so Ronin's failure block echoes back
`exec env … /bin/sh -ec "…"`, which the recogniser did not read for want of the
exact string `/bin/sh -c "`. `.4` and `.5` set `.SHELLFLAGS` again explicitly
(globally and per-target) and add a `-` recipe prefix. In every one, both tools
run the same script, it fails the same way, and GNU's half is already the
`recipe-error-line` family — with `(ignored)` where the `-` withdrew the status,
which Ronin also honours.

## options — 6 rows

* **`dash-t.diff`, `dash-t.diff.1` — narration (`touch-announce`).** GNU prints
  `touch interm-a`, `touch final-a`, … and `touch xxx`; Ronin prints its
  ordinary `[N/M]` progress lines. Measured: **the filesystem effects are
  identical** — the same files exist afterwards, at the same size (0 bytes; both
  tools touched rather than ran the recipe). This is the 2026-08-17 operator
  ruling on `make-archive-member-touch` exactly: *"the BEHAVIOUR lands as
  specified, the NARRATION does not."*
* **`dash-n.diff.4` — DEFECT, FIXED 2026-08-27**
  (`make-touch-touches-a-target-whose-recipe-expands-to-nothing`).
  Run `-t -n`, and deliberately *not* covered by `touch-announce`. The
  filesystem half was the real finding: under `-t` alone, GNU left both `a` and
  `b`, and Ronin left only `a`, because `b`'s recipe `$(FOO)` expands to a
  lone `+` and Ronin made no edge for a recipe that expands to nothing. A
  control probe with `b: c; @:` had Ronin touch both.

  The `+` turned out to be incidental — *any* recipe coming to nothing behaved
  that way — and the graph now carries whether the target HAS a recipe as
  against whether that recipe produced a command, which is the distinction `-t`
  turns on. The filesystem halves agree in all eight probed shapes. The row's
  remaining residue is narration: under `-t -n` GNU names the touch it would
  perform and Ronin names the recipe it stands in for. Nothing is written on
  either side.
* **`dash-n.diff.5` — DUPLICATE.** `-n` over `@$(MAKE) -f … bar`. GNU echoes
  the line *and runs it*, so the child's `echo n --no-print-directory` appears;
  Ronin does not. This is divergence #6 of `make-oracle-divergences.md` (`-n`
  does not run a `+`-marked or `$(MAKE)`-referencing recipe line), whose owner is
  `make-recipe-dry-run`; the row id is now recorded on that node.
* **`dash-k.diff` — suite artifact.** See above, along with the real `-k` defect
  the probe for it exposed.
* **`warn-undefined-variables.diff.2`, `.3` — DEFECT**
  (`make-a-warning-gnu-emits-about-a-makefile-is-emitted`). Ronin accepts
  `--warn-undefined-variables` and emits nothing. The build is identical in both
  tools; the whole difference is the two warnings GNU prints. Filed rather than
  narrated because the switch's *entire purpose* is the warning, so accepting it
  silently is an interface claim Ronin does not honour. See the node for the
  ruling this needs.

## Single rows

| Row | Case | GNU 4.4.1 | Ronin | Outcome |
| --- | --- | --- | --- | --- |
| `features/escape.diff.8` | `bar := $(call self,\#bar\#)` | `#foo# \#bar\#` | `#foo# #bar#` | **DEFECT** — `make-an-escaped-hash-in-a-function-argument-keeps-its-backslash`. Both agree on a bare `#` inside an argument; they disagree on an escaped one, which GNU leaves as written because the `#` was not opening a comment. |
| `features/order_only.diff.3` | `foo: bar \| baz` then `foo: baz` | `$\| =` (empty) | `$\| = baz` | **DEFECT** — `make-a-normal-prerequisite-outranks-the-same-name-as-order-only`. `$^` already agrees. |
| `features/order_only.diff.4` | same, second invocation | same | same | **DEFECT** — same node. |
| `features/parallelism.diff.6` | four failing jobs under `-j5` | `*** Waiting for unfinished jobs....` | (silent) | **narration** (`waiting-for-jobs`). Every remaining job's output — `fail.2`, `fail.3`, `OK` — is on both sides, so Ronin waited too. |
| `features/patternrules.diff.7` | `x.t2: dep` with `%.t1 %.t2:` | builds `dep` | never builds `dep` | **DEFECT** — `make-a-grouped-pattern-target-keeps-its-own-explicit-prerequisites`. A whole edge lost. |
| `features/recursion.diff` | `$(MAKE) -f … foo` twice, `-w -j 2` | echoes both recursive lines | composes them | **narration** (`recursive-invocation-echo`). |
| `features/recursion.diff.2` | `MAKEOVERRIDES += FOO+=bar` | `bar` | *(empty)* | **DEFECT** — `make-makeoverrides-reaches-a-recursive-invocation`. |
| `features/shell_assignment.diff.1` | `hash != printf '\043'` | `<bar> <#> <bar#baz>` | `<bar> <> <barbaz>` | **DEFECT** — `make-a-shell-assignment-keeps-a-hash-its-command-printed`. Ronin comment-strips the captured output. |
| `features/statipattrules.diff.3` | `foo: foo%: % %.x % % % y.% %` | builds `.x`, `y.`, `foo` | `ronin: empty path`, exit 2 | **DEFECT** — `make-an-empty-static-pattern-prerequisite-is-not-an-empty-path`. A refusal where GNU builds. |
| `features/utf8.diff` | makefile opens with a UTF-8 BOM | `all` | `﻿all` | **DEFECT** — `make-a-byte-order-mark-is-not-part-of-the-first-target`. |
| `features/vpath.diff.4` | `VPATH = vpa`; `%.x:` and `vpa/foo.x:` | `vpath vpa/foo.x` | `pattern foo.x` | **DEFECT** — `make-a-vpath-resolved-name-keeps-its-explicit-rule`. |
| `misc/general4.diff.1` | `foo: foo.o` with built-in rules | `cc foo.o -o foo` | `cc foo.c foo.o -o foo` | **DEFECT** — `make-a-link-rule-does-not-inherit-the-compile-rules-source`. The compile step agrees. |
| `variables/SHELL.diff.7` | `.SHELLFLAGS = -xec` | `*** [SHELL.mk.7:3: all] Error 1` | Ninja failure block | **narration** (`ninja-failure-block`). The `-x` trace lines `+ true` etc. appear identically on both sides. |
| `variables/define.diff.7` | `define NAME = $(NAME)` | warns `extraneous text after 'define' directive`, then `ok` | `ok` | **DEFECT** — `make-a-warning-gnu-emits-about-a-makefile-is-emitted`. Identical build; a diagnostic only. |
| `variables/define.diff.16` | `define = define` and friends | prints `define`, exit 0 | **panic** at `kati/src-rs/parser.rs:1125`, exit 101 | **DUPLICATE** — `make-a-define-directive-whose-name-reads-as-an-assignment-panics` (open). Row id recorded on the node. |
| `variables/special.diff.2` | `.RECIPEPREFIX` with backslash-newlines in recipes | echoes the recipe across its own physical lines | names the joined command in one `[N/M]` line | **narration, documented only.** Both hand the shell text that a POSIX shell reduces to the same words (`\`+newline is removed), and the recipes are all `:`. Not mechanised: recognising GNU's continuation fragments needs the makefile's recipe lines rejoined across `\`+newline first, and the classifier's recipe reader takes each tab-prefixed physical line as it stands. |

---

## What is left, and why

Fifty-eight rows remain `unclassified`, and that is deliberate for every one of
them:

* **36 defects.** They belong in `unclassified` until they are fixed. Each has a
  filed node with a reproducer. Nothing about them was accepted.
* **7 suite artifacts.** Not divergences. There is no family for "the directory
  had a leftover file", and inventing one would be a way of hiding a future real
  difference behind a plausible label.
* **7 narration rows that could not be mechanised safely.** Each says which
  obstacle stopped it — a bare-regex expectation, a switch that arrives through
  the environment, a multi-line command, a recipe rejoined across continuations,
  a version banner. All are explained here; none is a divergence of build
  intent.
* **5 GNU-internal.** The case asserts GNU's own implementation. Recorded rather
  than reclassified, so a later reader can disagree with the judgement without
  having to rediscover the case.
* **3 duplicates.** Owned elsewhere; the row ids are now recorded on the owning
  nodes so the count stays traceable.

**Undiagnosed: none.** Every one of the 85 was measured against the oracle.

The three probes that were built and then *disproved* are worth recording, since
each was an initially plausible defect: `features/patspecific_vars.diff.5`
(reproduced clean, both tools identical — the difference was `bar/`),
`variables/private.diff.7` (same), and `variables/automatic.diff.7` (identical in
every state tried, in both directions). The habit that caught all three was
recreating the case rather than reading the diff.
