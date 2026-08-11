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

Status: open

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

Acceptance: the reduced case MUST create `installed/value.h` repeatedly with
`-j16`; a clean Linux headers build MUST package every generated x86 UAPI
wrapper, including the three files above; and Necessary OS MUST advance past
`compiler-rt@seed` using that clean kernel-header package.
