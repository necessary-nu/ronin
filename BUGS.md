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
