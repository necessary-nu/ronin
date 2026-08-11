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
