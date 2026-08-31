# Bugs

## Linux rejects Ronin because `.FEATURES` omits `output-sync`

Status: fixed

Observed with Ronin revision `fcd4908612b058fb07b3e2e3db0d6173e3c4515f`
while bootstrapping Linux 6.18.2 for Necessary OS.

Ronin's Make front end reports compatibility with GNU Make 4.4.1, but its
`.FEATURES` value does not contain `output-sync`. The top-level Linux Makefile
uses that feature as its GNU Make >= 4.0 compatibility probe:

```make
ifeq ($(filter output-sync,$(.FEATURES)),)
$(error GNU Make >= 4.0 is required. Your Make version is $(MAKE_VERSION))
endif
```

At that revision, invoking the Linux tree through a symlink named `make`
therefore failed before it evaluated any target:

```text
ronin: Makefile:15: GNU Make >= 4.0 is required. Your Make version is 4.4.1.
```

GNU Make 4.4.1 accepts the same Makefile and proceeds. This blocked
`kernel-headers@seed`, the first Linux build after Necessary OS successfully
built `ronin@seed` and installed its `make` and `ninja` links.

Relevant implementation details at the affected revision:

- `kati/src-rs/evaluate.rs` sets `MAKE_VERSION?=4.4.1`.
- Make mode adds `jobserver` and `jobserver-fifo` to `.FEATURES`.
- `src/make/cli.rs` deliberately does not advertise `output-sync`, because
  `-O`/`--output-sync` is currently an accepted no-op.

Resolution: Ronin's Ninja engine already captures a command edge's output and
publishes it as one unit, providing always-on target-style output
synchronization. Make mode now advertises that capability as `output-sync`.
The `-O`/`--output-sync` mode selector remains an accepted no-op because Ronin
does not install a separate Make reporting path. The Linux guard above is a
regression test.

## Recursive Make subgraphs collide on local target names

Status: fixed

Observed with Ronin revision `71cb93ac5b95c5c1561441cb4642290fca2580da`
while retrying the Linux 6.18.2 Necessary OS build after the `output-sync` fix.
The version guard now passes, but the kernel build later stops with:

```text
ronin: multiple rules generate FORCE
```

This is not two explicit rules in one Makefile; Ronin handles that case. The
collision is between independent recursive Make invocations. A reduced case is:

```make
# Makefile
all: one two
one: ; +$(MAKE) -f one.mk
two: ; +$(MAKE) -f two.mk

# one.mk
all: one.out
one.out: FORCE ; @touch $@
FORCE:

# two.mk
all: two.out
two.out: FORCE ; @touch $@
FORCE:
```

At that revision, Ronin refused this graph with `multiple rules generate all`.
GNU Make runs the two child invocations successfully because each recursive
invocation has its own target namespace. Linux reaches the same defect through
the conventional `FORCE` target declared by many of its recursive Kbuild
Makefiles.

Resolution: each independently compiled recursive Make unit now allocates its
targets in a private graph namespace while retaining their real filesystem
paths for commands and freshness checks. Repeated target names within one unit
remain canonical, so genuine duplicate producers are still rejected. The
reduced case above is a regression test.

## An inherited exported variable keeps its stale value after reassignment

Status: fixed

Observed with Ronin revision `e7ca7976b4a929fef15326b70fe4cf3cf5028356`
while retrying the Linux 6.18.2 Necessary OS build after recursive target
namespaces were isolated. The `FORCE` collision is fixed, but a nested Kbuild
evaluation fails with:

```text
./scripts/Makefile.build:37: /scripts/Makefile: No such file or directory
ronin: No rule to make target '/scripts/Makefile'.
```

The reduced case has three Make invocations:

```make
# Makefile
export ROOT := inherited-before-child
all: ; +$(MAKE) -f one.mk

# one.mk
ROOT := .
all: ; +$(MAKE) -f two.mk

# two.mk
include $(ROOT)/included.mk
all: ; @printf '%s\n' 'ROOT=$(ROOT) VALUE=$(VALUE)'

# included.mk
VALUE := inherited
```

GNU Make retains the export attribute when `one.mk` replaces the
environment-origin value and passes `ROOT=.` to `two.mk`, which succeeds.
Ronin passes the original `ROOT=inherited-before-child` to the grandchild and
fails to include `inherited-before-child/included.mk`.

Linux hits the same shape. Its wrapper invocation exports `srcroot` before it
has a value, a child assigns the final `srcroot := .`, and a later recursive
`scripts/Makefile.build` must inherit that replacement. Ronin instead passes
the stale empty value, so `$(srcroot)/$(obj)/Makefile` becomes
`/scripts/Makefile`.

Expected: assigning a new file-origin value to an environment-origin exported
variable MUST preserve its export attribute, and semantic grandchildren MUST
receive the new value. Add the three-level case above as a regression test.

Resolution: Make compilation now treats an imported environment name as
implicitly exported when its evaluator binding is replaced or removed. It
publishes only that delta, so untouched inherited values retain their original
bytes rather than being re-expanded as Make syntax; GNU Make's special `SHELL`
handling remains unchanged. The three-level case now reaches `included.mk`, and
the grandchild receives `ROOT=.`.

## Makefile-assigned `MAKEFLAGS` became recursive goals

Status: fixed

Observed with Ronin revision `36860b702ba16a2adb4f834837b6d908a2ee05fa`
while building Linux userspace headers. Kbuild begins with:

```make
MAKEFLAGS += -rR
```

Ronin retained the evaluated variable as an ordinary string. Because command
line overrides already occupied the suffix after `--`, the semantic self-child
parsed the appended `-rR` as a goal. Its `MAKECMDGOALS` became `-rR headers`,
which selected Kbuild's configuration branch and made absent
`include/config/auto.conf{,.cmd}` regeneration roots. The resulting
`auto.conf` diagnostic was therefore downstream evidence, not an optional
include defect.

Resolution: Kati now calls the Make frontend after every effective global
`MAKEFLAGS` assignment. Ronin decodes the value through its existing GNU Make
option grammar, mutates the accumulated switch state, reapplies the
higher-precedence environment and command-line switches, and immediately
replaces `MAKEFLAGS` and `MFLAGS` with their canonical values. Command-line
assignments remain behind one recursive `-- $(MAKEOVERRIDES)` suffix. The final
state controls the current Ninja scheduler and is inherited by semantic
children.

A regression covers immediate canonicalization, current-build `-k`, and a
recursive child receiving `-rR` as switches rather than goals. The real Linux
`headers` build now compiles its complete graph and reaches recipe execution;
its next failure is the separately tracked `$?`/`KATI_NEW_INPUTS` lowering gap.

## `$?` lowering passes a phony prerequisite to `find`

Status: fixed

Observed with Ronin revision `bd54acc1bb5d6ddabba88295b14e12aae18bae1b`
while building Linux 6.18.2 userspace headers for Necessary OS. This is the
existing `make-newer-prerequisite-automatic-variable` gap promoted from corpus
residue to a bootstrap blocker.

A reduced case is:

```make
.PHONY: FORCE
PHONY := FORCE

question.out: question.in FORCE
	@printf 'newer=%s\n' '$(filter-out $(PHONY),$?)'
	@touch $@

FORCE:
```

With `question.in` present and `question.out` absent, GNU Make 4.4.1 prints
`newer=question.in` and succeeds. Ronin instead lowers `$?` to a shell-time
placeholder before `filter-out` can see its members, then generates:

```text
KATI_NEW_INPUTS=$(find question.in FORCE \
    $(test -e question.out && echo -newer question.out))
find: ‘FORCE’: No such file or directory
```

Linux uses the same shape through `newer-prereqs = $(filter-out $(PHONY),$?)`
and the conventional phony `FORCE` prerequisite. Its `make headers` graph now
compiles 1,037 edges, but the first `scripts/basic/fixdep` and syscall-header
recipes fail before their real commands run because `find FORCE` exits nonzero.

Expected: compile `$?` from the scheduler's ordinary prerequisites that are
newer than the target, preserving Make-level transformations such as
`filter-out` and never exposing `KATI_NEW_INPUTS` shell syntax. The reduced case
must agree with GNU Make, and Linux `make headers` must advance past the
generated-header wave.

Resolution: Kati now selects an explicit `$?` evaluation timing for each graph
destination. The Ninja manifest writer retains its recipe-shell fallback;
Ronin's direct graph sink leaves a typed scheduling-boundary reference and
carries any `filter-out` exclusions as edge metadata. After prerequisites have
settled, Ronin compares every ordinary prerequisite with the target snapshot.
Excluded prerequisites still make the edge run, but do not enter the value
substituted into the inline recipe or response script.

The two owned automatic-variable corpus cases now agree with GNU Make 4.4.1,
including an absent prerequisite whose rule materialises no file. A dedicated
regression pins the Linux `FORCE`/`$(filter-out $(PHONY),$?)` shape. A release
Ronin invoked through a symlink named `make` also reached all 1,052 Linux
`headers` edges in the reused `/data/cap9p/linux` tree, but that was not clean
verification: the ignored generated executable `scripts/unifdef` already
existed. The clean Necessary OS seed run confirms this `$?` gap is fixed and
exposes the separate recursive ordering defect below.

## A recursive child's prerequisites outrun its parent prerequisites

Status: fixed

Observed with Ronin revision `893ea90f462ceb42b72557b5aca229309444c60d`
while rebuilding Linux 6.18.2 userspace headers for Necessary OS with 16 jobs.
The earlier `$?` failure is fixed: `scripts/basic/fixdep` and the generated
syscall-header recipes now complete. The build instead stops when an indirect
prerequisite of a semantic recursive child starts before the recursive parent
target is eligible to run:

```text
./scripts/headers_install.sh: 41: scripts/unifdef: not found
```

The reduced case is:

```make
# Makefile
all: prepare
	+$(MAKE) -f child.mk child

prepare:
	@sleep 1
	@touch ready

.PHONY: all prepare

# child.mk
child: leaf

leaf:
	@test -e ready
	@touch leaf

.PHONY: child leaf
```

GNU Make 4.4.1 with `-j16` waits for `prepare`, enters the child, and succeeds.
Ronin 1.14.0 with `-j16` schedules `leaf` immediately, before `prepare` has
finished:

```text
[1/2] build leaf
FAILED: [code=1] leaf
cd '...' && env 'MAKELEVEL=2' /bin/sh -c "(test -e ready ) && (touch leaf )"
[2/2] build prepare
ronin: build stopped: subcommand failed.
```

Linux has the same dependency shape. Its top-level `headers` target depends on
`scripts_unifdef` and then invokes recursive Make twice. Each child default
goal depends on hundreds of header-install recipes, and those recipes call
`scripts/headers_install.sh`, which executes the `scripts/unifdef` produced by
the parent prerequisite. The child recipes therefore cannot start merely
because their graph was compiled.

`GraphSink::attach_child_ordering` currently adds the parent's prerequisites
as order-only inputs of each child *goal*. That delays completion of the goal,
but Ninja remains free to build the goal's own prerequisites in parallel with
those order-only inputs. Semantic recursion must instead preserve the process
boundary: no command in the recursively composed child invocation may start
before all ordinary and order-only prerequisites of the parent target have
completed. Consecutive recursive recipe lines must retain the same boundary
between child groups.

Acceptance: the reduced case succeeds repeatedly with `-j16`, including when
`leaf` is an indirect prerequisite rather than the child's goal recipe, and
Necessary OS's `kernel-headers@seed` completes its parallel Linux `headers`
build.

Resolution: Ronin now records every edge in a compiled recursive child unit,
including nested recursive units, and adds the recursive parent's ordinary and
order-only prerequisites to every edge in that subtree. Consecutive recursive
recipe lines likewise fence every edge in each later child group behind the
targets of the preceding group. The indirect-prerequisite regression succeeds
eight consecutive times with `-j16`.

Necessary OS run `1786468373998-736102` then built Ronin revision
`176a1c08fcb16babcd41bf44f311e298f351a476` and `kernel-headers@seed` in the
new output root `/data/pkg-build-ronin-boundary-176a1c0`: 2 packages built, 0
reused, and 1 bootstrap toolchain imported. The clean Linux build compiled
`scripts/basic/fixdep` at edge 22 and `scripts/unifdef` at edge 24 before the
header-install subtree, completed all 1,037 edges, and installed the userspace
headers. The retained build log is
`/home/brendan/.cache/necessary/runs/1786468373998-736102/logs/kernel-headers--x86-64-x86-64--seed-.log`.

## A recursive child is evaluated before its parent prerequisites run

Status: fixed

Observed with Ronin revision `176a1c08fcb16babcd41bf44f311e298f351a476`
in Necessary OS run `1786474273988-942846`, using 16 jobs. The recursive
subtree execution fence is fixed: Linux builds `scripts/unifdef` at edge 24
before starting its header-install edges and reports successful completion of
all 1,037 scheduled edges. The resulting `kernel-headers@seed` package is
nevertheless incomplete.

Linux's `archheaders` prerequisite generates wrapper files such as
`arch/x86/include/generated/uapi/asm/types.h`, `param.h`, and `ioctl.h`. The
later recursive invocation of `scripts/Makefile.headersinst` discovers those
files with `$(wildcard ...)` while evaluating the child Makefile. GNU Make
starts that child process only after `archheaders` has completed, so the files
enter the child graph and are installed. Ronin compiles the semantic child
before executing the parent graph. Its wildcard therefore sees none of the
generated wrappers, and no edges for their installed forms exist to fence.

The reduced case is:

```make
# Makefile
all: generate
	+$(MAKE) -f child.mk child

generate:
	@mkdir -p generated
	@printf '#define GENERATED 1\n' > generated/value.h

.PHONY: all generate

# child.mk
HEADERS := $(wildcard generated/*.h)
OUTPUTS := $(patsubst generated/%,installed/%,$(HEADERS))

child: $(OUTPUTS)

installed/%: generated/%
	@mkdir -p installed
	@cp $< $@

.PHONY: child
```

GNU Make 4.4.1 with `-j16` runs `generate`, evaluates the child, and creates
`installed/value.h`. Ronin with `-j16` reports only one successful edge:

```text
[1/1] build generate
```

It exits zero with `generated/value.h` present and `installed/value.h` absent.
This distinguishes child *evaluation* ordering from the fixed child *edge*
ordering: attaching prerequisites to every already-compiled child edge cannot
recover edges selected by files that did not exist when the child was
compiled.

In the Necessary build, archive
`08394f942c8808fed0a4d109057b18c14a19a1becc538c83aec16b09d5c59d0c.box`
contains `usr/include/linux/types.h` but omits `usr/include/asm/types.h`,
`usr/include/asm/param.h`, and `usr/include/asm/ioctl.h`. The next package,
`compiler-rt@seed`, consequently fails while compiling sanitizer sources:

```text
/sysroot/usr/include/linux/types.h:5:10: fatal error: 'asm/types.h' file not found
/build/src/compiler-rt/lib/sanitizer_common/sanitizer_linux.cpp:30:14:
fatal error: 'asm/param.h' file not found
/sysroot/usr/include/linux/ioctl.h:5:10: fatal error: 'asm/ioctl.h' file not found
```

The first staged-evaluation implementation, revision
`d97cca14878274b5d5d084ab22a812844adf7a00`, exposed a nested form of the same
boundary in clean Necessary OS run `1786476396058-1334673`. The outer
`headers` target needs `scripts_unifdef`, which is itself a held recursive Make
target. Kati's target walk presented `headers` before the held producer edge,
so the provisional graph requested `scripts_unifdef` before that edge had been
composed and stopped with `scripts_unifdef missing and no known rule to make
it`. A reduced nested-recursion regression reproduced the failure as a missing
`generate` prerequisite. Held recursive edges must therefore be composed in
stable dependency order as well as staged across their execution boundaries.

Acceptance: the reduced case MUST create `installed/value.h` repeatedly with
`-j16`; a clean Linux headers build MUST package every generated x86 UAPI
wrapper, including the three files above; and Necessary OS MUST advance past
`compiler-rt@seed` using that clean kernel-header package.

Resolution: revisions `d97cca14878274b5d5d084ab22a812844adf7a00` and
`e4dc77a102f0d755448978a11618f0c8d5d30304` stage recursive child evaluation
behind the parent's completed prerequisites and put held recursive producer
edges before consumers that need their targets. The original reduced case runs
eight consecutive times with `-j16`; the nested recursive-prerequisite case is
also covered directly.

Necessary OS run `1786477483250-1550720` built the final revision in the new
output root `/data/pkg-build-ronin-eval-e4dc77a`: 5 packages built, 0 reused,
and 1 bootstrap toolchain imported. Linux built `scripts/unifdef`, completed
the 963-edge generic header phase and the separate 68-edge x86 phase, and
installed `usr/include/asm/types.h`, `param.h`, and `ioctl.h`. Injecting and
listing the resulting
`kernel-headers-6.18.2-x86_64-seed.box` (BLAKE3
`11c8bd9f8545d864262a875766ce93b37a3b012689a18f10fba1393d1ae508c2`)
confirmed all three paths are in the package. `compiler-rt@seed` then consumed
that package, completed its 375-edge build, and installed successfully. The
retained package log is
`/home/brendan/.cache/necessary/runs/1786477483250-1550720/logs/kernel-headers--x86-64-x86-64--seed-.log`.

## A changed recipe command rebuilds an otherwise up-to-date Make target

Status: fixed in `2dc68b0`

Observed with Ronin revision `e4dc77a102f0d755448978a11618f0c8d5d30304`
while building the ICU4X-backed musl libc for Necessary OS with 16 jobs.

Ronin persists Ninja's command hashes between separate Make invocations and
uses a changed expanded recipe as a reason to rebuild a target. GNU Make does
not: freshness is determined from the target and prerequisite timestamps, not
from whether command-line variables would expand its recipe differently in a
later invocation.

A reduced case is:

```make
all: out

out:
	printf '%s\n' '$(VALUE)' > $@

install: out
	cp out installed
```

Run the two invocations in the same directory:

```sh
make VALUE=kept
make install
```

GNU Make 4.4.1 builds `out` only in the first invocation; both `out` and
`installed` contain `kept`. Ronin rebuilds `out` in the second invocation
because `VALUE` is then empty:

```text
[1/1] build out
[1/2] build out
[2/2] build install
```

Both files consequently become empty even though no target or prerequisite
timestamp made `out` stale.

Necessary OS hits the same difference in musl's conventional build/install
sequence. The build invocation passes the ICU4X archive through
`EXTRA_LIBS=/build/work/target/x86_64-unknown-linux-musl/release-musl/libposix_locale_icu4x.a`
and successfully links it into `lib/libc.so`. The following `make install
DESTDIR=/staging` invocation should only copy that up-to-date library. Ronin
instead recompiles and relinks it with the second invocation's empty
`EXTRA_LIBS`, then fails on every `__icu4x_*` reference. Necessary OS run
`1786480330604-805720` captured both expanded shell invocations and the failing
install edge.

Expected: Make mode MUST ignore persisted Ninja command-hash differences when
deciding whether an existing target is dirty. The reduced second invocation
must run only the `install` recipe and preserve `kept`. Ninja mode's native
command-change behavior is unaffected.

Resolution: Kati now marks every Make recipe rule with Ninja's `generator = 1`
control in both its direct graph and retained manifest. That keeps Make
freshness timestamp-only without adding Make provenance to the executor, while
native Ninja rules retain their ordinary command-hash behavior. Five timestamp
cases moved from discovery into the GNU Make build-intent gate, and the reduced
case above passes as an integration test.

The Necessary OS definition of done passed in run `1786484875353-377057` using
Ronin `2dc68b0` and a temporary recipe with the musl install workaround removed.
The build linked `lib/libc.so` once at edge 1369/1369 with the ICU4X archive;
the following bare `make install AR=llvm-ar RANLIB=llvm-ranlib
DESTDIR=/staging` copied it at edge 3/235 instead of relinking it. `musl@seed`
completed successfully, `sysconf` remained exported, and there were no
undefined `__icu4x_*` symbols. The retained log is
`/home/brendan/.cache/necessary/runs/1786484875353-377057/logs/musl--x86-64-x86-64--seed-.log`.

## Command-line variables disappear in a shell-loop recursive Make

Status: fixed

Observed with Ronin revision `1b732cffef97683d8253b6ed47e1261d61ec7eca`
while building Gawk 5.3.1 in Necessary OS run `1786491904052-2520395`.

Ronin propagates a command-line variable to a directly recognised recursive
Make, but loses it when `$(MAKE)` is invoked from a compound shell recipe. A
reduced case is:

```make
# Makefile
all:
	@for dir in sub; do \
		(cd $$dir && $(MAKE) print); \
	done

.PHONY: all

# sub/Makefile
VALUE = file-default

print:
	@printf 'VALUE=%s\n' '$(VALUE)'

.PHONY: print
```

Invoked with the assignment after the goal, matching the package driver:

```sh
make -j16 all VALUE=
```

GNU Make 4.4.1 prints `VALUE=`. Ronin instead executes the recursive process
with no effective command-line override and prints `VALUE=file-default`:

```text
[1/1] for dir in sub; do (cd $dir && .../make print); done
[1/1] printf 'VALUE=%s\n' 'file-default'
VALUE=file-default
```

Gawk's Automake-generated top-level install target uses the same shell-loop
shape. Necessary OS invokes `make install profile_DATA=` to suppress two
irrelevant profile snippets, but the `extras` child receives its Makefile
default, `profile_DATA = gawk.sh gawk.csh`. With `--prefix=/usr`, Automake's
default `sysconfdir = ${prefix}/etc` puts them in `/usr/etc/profile.d`, and the
package layout validator rejects the result. The retained log is
`/home/brendan/.cache/necessary/runs/1786491904052-2520395/logs/gawk--x86-64-x86-64--seed-.log`.

Expected: every semantic recursive Make receives the parent's command-line
variable assignments through GNU-compatible `MAKEFLAGS`/`MAKEOVERRIDES`, even
when the recursive invocation runs inside a shell loop or compound recipe. The
reduced case MUST print `VALUE=` repeatedly with `-j16`.

Resolution: each compiled Make unit now exports its final canonical
`MAKEFLAGS` and `MFLAGS` values to every recipe, so a real recursive process
inside a shell loop receives the command-line override table. The reduced
shell-loop case is an integration test and prints `VALUE=` without falling back
to `file-default`.

Corrected 2026-08-31. This entry claimed the jobserver authorization was
spliced into those values before execution. It was not: the splice
(`Transport::publish_into`) is real and unit-tested, but its only Make-mode
call site runs over an empty base environment, and Make mode set
`serve_jobserver = false` outright, so there was no authorization to splice.
A shell-loop recursion therefore received the width and no budget, and ran
`-j` of its own beside its parent's. The authorization now travels the way GNU
Make sends it — through the switch table that rebuilds `MAKEFLAGS`, settled
before the read — and one budget bounds the tree. See
`[spec:ronin:req:make.jobserver+2]`.

## A shell-computed recursive assignment is rejected as non-static

Status: fixed

Observed with Ronin revision `1b732cffef97683d8253b6ed47e1261d61ec7eca`
while building Zstd 1.5.7 in Necessary OS run `1786492219209-3222893`.

Zstd recursively builds a configuration-specific object directory whose name
is a hash computed in the recipe:

```make
libzstd.a:
	+$(MAKE) --no-print-directory $@ \
	  BUILD_DIR=obj/conf_$(shell printf '%s' '$(CFLAGS) $(CPPFLAGS)' | md5sum | cut -f1 -d' ')
```

Kati deliberately preserves that `$(shell ...)` as shell command substitution
until the recipe boundary. Ronin's semantic recursive-Make parser instead
required every word to be static and refused the package before constructing
the child graph:

```text
ronin: recursive Make recipe is not a static invocation: ...
BUILD_DIR=obj/conf_$(echo ... | md5sum | cut -f 1 -d " ")
```

Resolving the command exposed a second evaluator defect in Zstd's two object
suffix passes: a word not matching `.S` still had `.o` appended, producing
`debug.o.o` instead of retaining `debug.o`.

Expected: shell substitutions in an otherwise composable recursive invocation
run with the recipe's shell, flags, directory, and environment after its
prerequisites settle. Their resulting words form the child Make arguments.
Suffix-substitution references replace only matching words.

Resolution: recursive invocation resolution now evaluates shell substitutions
at the scheduler's compilation boundary, applies shell quoting and field
splitting, and rejects only results that remain dynamic. Kati's substitution
reference implementation now preserves nonmatches and routes percent patterns
through its general pattern substitution. Focused regressions cover both
behaviours. The retained Zstd source tree now compiles to the correct computed
`conf_b7fa5eb4cb1f6c5f21930f5277aefbd8` directory and complete 37-edge
`libzstd.a` dry-run graph.

## A stale build-log mtime overrides an up-to-date Make symlink

Status: fixed

Observed with Ronin revision `1b732cffef97683d8253b6ed47e1261d61ec7eca`
while installing Findutils in Necessary OS run `1786492219209-3222893`.

Findutils generates `locate/dblocation.texi`, then another recursive group
uses an existing link to it:

```make
dblocation.texi: ../locate/dblocation.texi
	$(LN_S) ../locate/dblocation.texi $@
```

After the referent is regenerated, `stat` follows the link and reports exactly
the prerequisite's mtime. GNU Make therefore considers the link current.
Ronin also observed those equal filesystem timestamps, but then compared the
prerequisite with the older mtime persisted when the link recipe first ran.
It reran `ln -s` against the existing link and failed:

```text
[1/1] ln -s ../locate/dblocation.texi dblocation.texi
ln: Already exists
```

Expected: persisted Ninja state may provide timing, dependency, and tooling
data for Make graphs, but it must not override Make's filesystem-only target
freshness. Native Ninja graphs retain their build-log-aware semantics.

Resolution: graph edges now carry a typed freshness-history policy. Ninja
edges default to `BuildLogAware`; the direct Make graph marks every edge
`FilesystemOnly`. The build log remains Ninja-compatible and continues to be
written, but its recorded mtime no longer dirties an otherwise current Make
target. A two-invocation symlink regression reproduces the stale-log shape,
and the retained Findutils target with its original stale `.ninja_log` now
reports `ronin: no work to do.`

## GNU Make's built-in compile variables are missing

Status: fixed

Observed with Ronin revision `6147c84b1a0f5d0116cf86ace7f6028c8de98281`
while building Zstd 1.5.7 at `zstd@bootstrap` in Necessary OS run
`1786524003873-3677276`.

Zstd's explicit object rules are written in terms of GNU Make's built-in
variables:

```make
$(ZSTD_DYNLIB_DIR)/%.o : %.c | $(ZSTD_DYNLIB_DIR)
	$(COMPILE.c) $(DEPFLAGS) $(OUTPUT_OPTION) $<

$(ZSTD_DYNLIB_DIR)/%.o : %.S | $(ZSTD_DYNLIB_DIR)
	$(COMPILE.S) $(OUTPUT_OPTION) $<
```

Ronin defines `CC`, `CXX`, and `AR` in its bootstrap Makefile, but not
`COMPILE.c`, `COMPILE.S`, or `OUTPUT_OPTION`. Consequently the C recipe loses
both its compiler and output argument and becomes, for example:

```text
(MMD -MP /build/work/lib/common/debug.c)
/bin/sh: 1: MMD: not found
```

The assembly recipe becomes the source pathname alone and fails by trying to
execute it:

```text
(/build/work/lib/decompress/huf_decompress_amd64.S)
/bin/sh: 1: /build/work/lib/decompress/huf_decompress_amd64.S: Permission denied
```

A reduced case is:

```make
all: hello.o

hello.o: hello.c
	$(COMPILE.c) $(OUTPUT_OPTION) $<
```

With a valid `hello.c`, GNU Make 4.4.1 expands the recipe to the equivalent of
`cc -c -o hello.o hello.c` and succeeds. Ronin expands it to `hello.c` and
tries to execute the source file.

Expected: unless `-R`/`--no-builtin-variables` is in effect, Ronin MUST expose
GNU Make's standard built-in variable catalogue, including the composition
variables used by explicit recipes (`COMPILE.c`, `COMPILE.S`, `OUTPUT_OPTION`,
and their constituent defaults). `-R` must continue to suppress them. The
reduced case and Zstd's real `libzstd` graph must both compile through Ronin.

Resolution: kati now installs GNU Make 4.4.1's `default_variables[]` catalogue
at its evaluation-initialization boundary, after the environment and before any
Makefile, as recursive bindings at the `default` origin. `.POSIX:` substitutes
the standard's values where GNU Make's `check_specials` does. `-R` withholds
the catalogue, and a Makefile's own `MAKEFLAGS += -rR` withdraws it once the
read is over, so a `$(origin CC)` on the next line still answers `default` and
the recipe that runs afterwards expands to nothing — which is what the Linux
kernel's build relies on. Six recorded build-intent cases cover the reduced C
recipe, the assembly recipe, Makefile override, origin and flavour, `-R`, and
the deferred `MAKEFLAGS` withdrawal, and the two previously diverging
variable-default cases now match. GNU Make's own `options/dash-r` case for a
Makefile-set `-R` became byte-identical in the upstream inventory. The retained
Zstd tree replays to a complete 76-edge dry-run graph whose object recipes are
real `clang … -c -MMD -MP -o …` and `clang … -c -o …` commands, and builds a
1.2 MB `libzstd.a` through Ronin's scheduler.

## Automake's maintainer rule regenerates `Makefile.in` without automake

Status: fixed

Observed with Ronin revision `aabc300c868dbc397d5766720b3b09ea0bae9b55`
while building PCRE2 10.47 at `pcre2@final` in Necessary OS run
`1786539432179-211696` (81 built, 1 failed, 64 skipped; PCRE2 was the sole
failure).

At edge `[32/46]` Ronin ran Automake's `Makefile.in` maintainer regeneration
recipe. Released tarballs ship a pregenerated `Makefile.in` and the build
sandbox correctly has no automake, so the `missing` shim failed:

```text
[32/46] for dep in ${KATI_NEW_INPUTS}; do ... automake-1.16 --foreign Makefile
FAILED: [code=127] Makefile.in
/build/src/missing: 81: automake-1.16: not found
WARNING: 'automake-1.16' is missing on your system.
```

GNU Make 4.4.1 does not run that rule. The cause is not timestamps: in the
extracted tarball `Makefile.in` (mtime 1760960108) is strictly newer than
every prerequisite named in the rule text — `aclocal.m4` (1760960106),
`configure.ac` and `m4/*.m4` (1760960102 and 1760960105). It is the rule's
shape. `configure` reported `whether to enable maintainer-specific portions
of Makefiles... no`, so it substituted `@MAINTAINER_MODE_TRUE@` with `#`:

```make
$(srcdir)/Makefile.in: # $(srcdir)/Makefile.am  $(am__configure_deps)
	@for dep in $?; do \
	...
```

The `#` opens a comment, so the rule has *no* prerequisites at all. A
single-colon rule with no prerequisites is up to date as soon as its target
exists, which is precisely how Automake disables maintainer rebuilds. Kati
strips that comment correctly; the defect was in Ronin's graph layer.

`src/graph/deferred.rs` decides freshness for *deferred* edges — those whose
recipe references `$?`, which Ronin lowers to `KATI_NEW_INPUTS` and resolves at
the scheduling boundary. `recompute_deferred_freshness` forced the edge dirty
whenever it had no ordinary prerequisites:

```rust
let mut timestamp_dirty = all_inputs_new || edge_data.non_order_only_inputs().is_empty();
```

That clause states GNU's double-colon rule — a `::` rule with no prerequisites
runs every time — but it was applied to every deferred edge, so any
single-colon rule with no prerequisites whose recipe mentions `$?` ran on every
invocation even with its target present and `$?` empty. Automake's maintainer
rule is exactly that shape, and remaking `Makefile.in` then cascaded into the
`Makefile: $(srcdir)/Makefile.in $(top_builddir)/config.status` rule.

A reduced case is:

```make
all: target

target: # prereq
	@echo RAN "[$?]"
```

With `target` present, GNU Make 4.4.1 reports `Nothing to be done for 'all'`.
Ronin ran the recipe. Deleting the `$?` from the recipe made Ronin agree,
which isolates the deferred path rather than comment parsing.

Expected: an empty ordinary prerequisite list forces a run only for rules Make
never considers current — phony targets, and double-colon rules that declared
no prerequisites. Every other prerequisite-free rule is up to date once its
target exists, whether or not its recipe reads `$?`. Double-colon rules with no
prerequisites must keep running every time.

Resolution: the empty-prerequisite clause is now gated on the edge's
`always_dirty` flag, which kati already derives as
`node.is_phony || node.unconditional_double_colon`, itself
`is_double_colon && has commands && no inputs && no order-only inputs`. The
signal existed; the graph was ignoring it. Five recorded build-intent cases
cover the prerequisite-free `$?` rule, the commented-prerequisite Automake
shape with a deliberately newer `dep`, the double-colon prerequisite-free rule
that must still run, a strictly newer prerequisite that must still fire and
name itself in `$?`, and equal timestamps that must not. The first two fail
against the previous binary and pass now. The retained PCRE2 tree replays from
46 scheduled edges to 44: both the `automake-1.16` regeneration and its
cascaded `config.status Makefile` remake are gone, and GNU Make 4.4.1 on the
same tree schedules neither. In GNU Make's own suite,
`variables/EXTRA_PREREQS.diff.1` moved from `ninja-progress` to
`no-work-line`, because with `all`, `tick` and `tack` all present as files —
the state the suite's shared working directory produces — GNU Make says
`'all' is up to date` and Ronin now says `no work to do` instead of rerunning
the recipe.

## Recipe lines expand before their rule runs, freezing `$(shell)` results

Status: fixed

Observed with Ronin revision `c35b0fdb8b16afd6ba5d6a1acbd472d88997cc49`
while building CPython (`python@seed`) for Necessary OS.

GNU Make expands a recipe line when the recipe executes. A recursively
expanded variable whose value contains `$(shell ...)` therefore re-runs the
shell command at each expansion, and a recipe that references it after a
prerequisite created a file sees that file. Ronin expands the recipe line
while constructing the build graph, so the `$(shell ...)` runs once, before
any rule has executed, and the frozen result is what every recipe sees.

Minimal reproduction:

```make
PROBE = $(shell test -f marker && echo found-`cat marker`)

all: marker
	@echo "probe says: [$(PROBE)]"

marker:
	@printf yes > marker
```

GNU Make 4.4.1 prints `probe says: [found-yes]`. Ronin prints
`probe says: []`, and its own transcript shows the command already expanded
before the `marker` rule ran:

```text
[1/2] printf yes > marker
[2/2] echo "probe says: []"
```

The real-world failure is CPython's cross-build support. `configure` bakes
this into `PYTHON_FOR_BUILD`:

```
_PYTHON_SYSCONFIGDATA_PATH=$(shell test -f pybuilddir.txt && echo $(abs_builddir)/`cat pybuilddir.txt`)
```

`pybuilddir.txt` is written by an early rule; under GNU Make every later
`$(PYTHON_FOR_BUILD)` expansion finds it. Under Ronin the variable freezes to
empty before the file exists, and the `checksharedmods` step dies with:

```text
ModuleNotFoundError: No module named '_sysconfigdata__linux_x86_64-linux-musl'
```

This failed `python@seed` and skipped the 129 nodes behind it, including
systemd and the system package. Suspected surface: the memoized Make graph
evaluation — a command recorded into the graph at construction time cannot
honour execution-time expansion; recipe lines (or at least their `$(shell)`
segments) need their expansion deferred to when the edge is scheduled.

Mechanism: kati compiled every reachable recipe into command text while it
built the graph, because that is what writing a `build.ninja` needs. Make mode
runs the graph in the process that compiled it, so it does not need that, and
now says so: `BuildSink::recipe_expansion` is `Launch` for the direct graph
sink and `Construction` for the manifest writer. A recipe the compiler does not
have to read for itself is left unexpanded, and the engine asks the front end
for the command as it launches the edge — after the edge's prerequisites are
built, and not at all for a target that turns out to be up to date. The
recipes the compiler still reads while compiling are the ones whose text
decides the graph's shape: a recursive `$(MAKE)` line, an automatic or declared
depfile, a `$?` the scheduler binds, a grouped double-colon action.

Reduced case, as a regression: `tests/make/recipe-variable-shell-runs-when-the-recipe-runs`
is the reproduction above, recorded from GNU Make 4.4.1. Five more cover the
rest of the timing: `recipe-of-a-current-target-is-not-expanded`,
`recipe-error-in-a-current-target-does-not-fire`,
`recipe-expansion-sees-an-earlier-recipes-file`,
`recipe-lines-are-expanded-before-the-first-line-runs`, and
`dry-run-expands-the-recipe-it-would-run`.

Replay: the failed `python@seed` tree survives at
`/data/pkg-build/work/python--x86-64-x86-64--seed-/build.overlay/0/upper`. Its
generated `Makefile` cannot be run outside the package sandbox — it bakes
absolute `/build` paths and remakes itself from a `srcdir` that exists only
there — so the replay takes CPython's own `PYTHON_FOR_BUILD` line and
`pybuilddir.txt` rule from that Makefile verbatim, with a stand-in for the
interpreter that records the `_PYTHON_SYSCONFIGDATA_PATH` it was given. GNU
Make 4.4.1 and Ronin now record the same two values: empty for the
`pybuilddir.txt` rule's own recipe, which is expanded before the file exists,
and the built `$(abs_builddir)` path for the `checksharedmods` recipe,
which is expanded after it.

## pcre2test links before libpcre2-posix.la finishes under -j

Status: fixed

Observed with Ronin revision `c35b0fdb8b16afd6ba5d6a1acbd472d88997cc49`
(plus the lazy-recipe-expansion repin) while building pcre2 10.47 for
Necessary OS.

The build fails at the `pcre2test` link:

```text
[46/49] echo "  CCLD    " libpcre2-posix.la;... --mode=link ... -o libpcre2-posix.la ...
  CCLD     libpcre2-posix.la
[47/49] ... --mode=link ... -o pcre2test src/pcre2test-pcre2test.o libpcre2-8.la libpcre2-posix.la ...
FAILED: [code=1] pcre2test
clang: error: no such file or directory: './.libs/libpcre2-posix.so'
```

The install step that runs moments later finds `.libs/libpcre2-posix.so.*`
present and installs the library normally, so the artifact exists almost
immediately after the failed link — the `pcre2test` edge ran while (or
before) the `libpcre2-posix.la` edge completed, where GNU Make orders them
strictly.

The generated rule chain (pcre2 `Makefile.in`, automake output):

```make
am__DEPENDENCIES_1 =
@WITH_PCRE2_8_TRUE@am__append_37 = libpcre2-8.la libpcre2-posix.la   # line 136
pcre2test_DEPENDENCIES = $(am__DEPENDENCIES_1) $(am__append_37) \    # line 517
	$(am__append_38) $(am__append_39) $(am__DEPENDENCIES_7)
pcre2test$(EXEEXT): $(pcre2test_OBJECTS) $(pcre2test_DEPENDENCIES) $(EXTRA_pcre2test_DEPENDENCIES)  # line 2067
```

Two minimal imitations of that shape were tried against this revision and
**both behave correctly** — Ronin waits and the build passes — so the
defect is not the obvious one:

1. A prerequisite list built from a defined-empty variable plus
   conditional-append variables (`$(am__DEPENDENCIES_1) $(am__append_37)
   ...`), with a slow library rule and a fast consumer.
2. The same, adding automake's remaining texture: `$(EXEEXT)` in the
   target name, an `_OBJECTS` variable chain, an empty
   `EXTRA_*_DEPENDENCIES`, undefined appends in the middle of the list,
   and a trailing space after the prerequisite list.

Suggested next step from the maintainer's side: dump the constructed graph
for the pcre2 tree and check whether `pcre2test` carries the
`libpcre2-posix.la` input edge at all. If the edge exists, the defect is
in scheduling or completion detection rather than parsing; if it is
absent, the drop involves something the imitations above do not capture
(the full automake preamble, `.SUFFIXES`/pattern interaction, or the
dirstamp machinery are the untested remainder).

This failed `pcre2` and skipped 64 nodes behind it, including sudo-rs and
systemd.

Resolution: it was not a race, and the suggested next step answered the
question the other way. The `pcre2test` edge does carry the
`libpcre2-posix.la` input and Ronin does order the two strictly. What put the
link where it could lose was that the link had no business running at all.

Ronin propagated "this edge's recipe ran" as "this edge's outputs changed".
GNU Make does not: it stats a target it has just remade and lets the timestamp
it then finds decide what the targets reading it do. Every autoconf tree
carries `src/config.h: src/stamp-h1`, whose recipe is `@test -f $@ || ...` —
it runs on every invocation, because `stamp-h1` is permanently newer, and it
moves nothing. Under the old reading that single no-op cascaded a full rebuild
of the whole tree, every invocation: a second `make -j8` on a finished pcre2
reran all 42 edges where GNU Make reran none. `make install` therefore carried
that rebuild along with the install recipes in one graph — 91 edges where GNU
runs 11 — and among them `install-libLTLIBRARIES` has libtool relink the
shared library, which removes `./.libs/libpcre2-posix.so` for a few
milliseconds. The `pcre2test` link, which should have been settled long before
and was present only because of the spurious rebuild, was in that graph
looking for exactly that file.

The fix is Ronin's graph engine; kati is unchanged. A Make command edge
carries `outputs_reobserved`, and once such an edge's command has run the
engine hands the question back to the timestamp comparison instead of
asserting a change. Two targets are not re-observed, because neither is a file
an answer can be read off: a phony one, and one the recipe left absent, which
GNU reads as infinitely new. The property is deliberately wider than Ninja's
`restat`, which grants the same outcome only to an output whose mtime did not
move at all: GNU also spares the reader of a target that moved backwards, or
forwards but still short of it. Eight cells of GNU's behaviour are recorded as
build-intent cases, `tests/make/target-remade-*` among them, and
`a_stamp_recipe_that_moves_nothing_rebuilds_nothing` holds the shape above
across three invocations.

On the reconstructed pcre2 10.47 the from-scratch build runs the same 45 edges
it did before, the second invocation runs the one no-op recipe and nothing
else — which is what `make --trace` shows GNU Make 4.4.1 doing on the same
tree — and `make -j8 install` runs 11 install recipes with no compile or link
edge among them.

## $(file ...) is refused in rules; Linux headers_install needs the read form

Status: fixed

Observed with Ronin revision `2340cd4c6ababb3380f4ae87ba3eee9e74a45508`
building `kernel-headers@seed` (linux-6.18.2 `headers_install`) for
Necessary OS:

```text
ronin: /build/linux/Makefile:1337: $(file ...) is not supported in rules.
```

The refusal is Ronin's own message, and the feature it names is load-bearing
for the kernel tree. The expansion chain that lands `$(file ...)` in recipe
context:

```make
# scripts/Kbuild.include:72 — the *read* form
read-file = $(subst $(newline),$(space),$(file < $1))

# Makefile:379 — recursively expanded, so it expands wherever it is used
KERNELRELEASE = $(call read-file, $(objtree)/include/config/kernel.release)

# Makefile ~1337-1340 — recipes whose filechk bodies reference KERNELRELEASE
include/generated/utsrelease.h: include/config/kernel.release FORCE
	$(call filechk,utsrelease.h)
```

`filechk_utsrelease.h` interpolates `$(KERNELRELEASE)` into its shell text,
so expanding the recipe requires evaluating `$(file < ...)` — a read, not a
write. GNU Make has supported `$(file <)` since 4.2 (and `$(file >)` since
4.0), both in any expansion context including recipes.

This is a regression relative to the pre-delivery-series Ronin: the same
kernel tree built `kernel-headers` successfully under the revision pinned
before the evaluator-scope/argument/rule-catalogue work landed. It blocks
`kernel-headers@seed`, and everything in the world sits above kernel-headers.

At minimum the read form needs real support — open the file, substitute its
contents, empty-string when absent per GNU semantics. The write forms
(`$(file >...)`, `$(file >>...)`) are what the "not supported in rules"
refusal presumably exists for; if writing stays unimplemented, the refusal
should distinguish the two rather than reject reads it could serve.

Resolution: the regression claim above is wrong, and that has to be said before
anything else, because it sent the search for a cause into work that had
nothing to do with it. The refusal is upstream kati's, `src/func.cc:934`:

```c++
void FileFunc_(const std::vector<Value*>& args, Evaluator* ev, ...) {
  if (ev->avoid_io()) {
    ev->Error("*** $(file ...) is not supported in rules.");
  }
```

It came across into the Rust port as `if ev.avoid_io` in `file_func_impl` and
was never touched afterwards. The bare `avoid_io` guard is there at every
revision this entry could mean — `8c269f6`, `1cd853c`, `10f2e0c`, `a7b9da0` —
so the evaluator-scope, argument and rule-catalogue work landed beside it
rather than causing it. `MAKE_VERSION` answered `4.4.1` at all of them too, and
`scripts/Kbuild.include` gates on
`ifneq ($(filter-out 4.0 4.1,$(MAKE_VERSION)),)`, so Kbuild took the
`$(file <)` branch rather than the `$(shell cat)` fallback throughout. Whatever
let an earlier revision finish `kernel-headers`, it was not a `$(file ...)`
that was served.

What the refusal actually is: a manifest writer's, stated at the evaluator.
`avoid_io` means a recipe is being compiled into text that some later run of
some other program will execute, and in that position the function genuinely
cannot be honoured — a write would land while the manifest is written instead
of while the build runs, and a read would answer from a tree the build has not
made yet. That is a true sentence about a destination. kati had one
destination, so it was written as a fact about the function.

Ronin has two, and `$(shell)` in a recipe was already split along that line.
This is the same split for `$(file ...)`: `FileEvaluation { Refused,
Expansion }` on `BuildSink`. kati's `--ninja` manifest writer answers `Refused`
and prints the same refusal it always did, which is the right answer for it.
kati's own executor answers `Expansion`. Ronin's `GraphSink` answers
`Expansion`, because Ronin runs the build in the process that expanded the
recipe — the position GNU Make is in when it expands one.

Both forms are served there, not only the read the kernel needs.
`file_read_func` is now GNU's `func_file`: one trailing line terminator
removed and no more, a carriage return taken with it so a CRLF file reads back
as its last line, an absent file read as nothing rather than as an error, a
directory reported against the `read:` rather than the `open:` because opening
one succeeds and reading it does not. A write happens before the recipe's first
line runs and is visible to a `$(wildcard)` further down that same recipe,
which is what GNU's `++command_count` in `func_file` is for. Six cases record
it against the oracle, `feature-file-read-reaches-a-recipe-through-a-recursive-variable`
among them — the kernel's `read-file`/`KERNELRELEASE`/`filechk` chain reduced,
byte-identical to GNU Make 4.4.1. GNU's own suite agrees from the other side:
`functions/file` case 21 is `all:;$(info $(file <  out1  ))`, and it moves out
of the upstream inventory's unclassified bucket into narration, where the whole
of the remaining difference is Ninja's progress line.

One thing is traded rather than won. A recipe Ronin cannot hold unexpanded —
one naming `$?`, one feeding a depfile, a recursive child's, a grouped
target's — is expanded while the graph is built, and its file operation now
happens there instead of stopping the build. For a target that is already up to
date and never runs, GNU performs no operation at all while Ronin performs one,
writing the unresolved `${KATI_NEW_INPUTS}` marker where `$?` should be. That
is strictly better than the hard stop it replaces and strictly worse than
silence: a refusal became an early write.
`feature-file-write-in-a-recipe-that-does-not-run` records it with a
`divergence` sidecar, and `make-recipe-file-operation-at-launch-only` owns
closing it. A recursive child's recipes carry the same trade `$(shell)` already
carries there, under `make-recursive-child-recipes-expand-at-launch`.

Fixed here is the refusal. Whether `kernel-headers@seed` now completes is for
the next world build to say, and the entry below is the question standing
between the two.

## A `headers_install` goal reaches a rule only `archprepare` wants

Status: open

Left over from the entry above, which explained the refusal without explaining
what met it.

The refusal fired at the Linux Makefile's `include/generated/utsrelease.h`
recipe while `kernel-headers@seed` built `headers_install`. Nothing on that
goal's chain asks for that file. From the same kernel tree:

```make
headers_install: headers
headers: $(version_h) scripts_unifdef uapi-asm-generic archheaders

archprepare: outputmakefile archheaders archscripts scripts \
	include/config/kernel.release asm-generic $(version_h) \
	include/generated/utsrelease.h include/generated/compile.h ...

include/generated/utsrelease.h: include/config/kernel.release FORCE
	$(call filechk,utsrelease.h)
```

`$(version_h)` is `include/generated/uapi/linux/version.h` — a different file
with a different recipe. `utsrelease.h` is reachable from `archprepare` and
from nothing the goal names, so a compiler that expands only what its goal
reaches should never have read that line.

Two readings, wanting different fixes. Either Ronin compiles recipes it has no
goal for, which is a defect in its own right that `$(file ...)` merely made
audible; or the kernel reaches `archprepare` from `headers_install` by a route
the top-level Makefile does not show, in which case there is nothing to fix and
this note is the answer. It is also the likelier explanation for the earlier
revision that finished this build than any change to `$(file ...)`, since the
refusal was present at that revision too.

Deciding it wants the constructed graph for a `headers_install` goal dumped and
compared against `make -n headers_install` on the same tree, which has not been
done. With `$(file ...)` served the line expands either way, so this is now a
question about what work is being done rather than about whether the build
stops.
