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
