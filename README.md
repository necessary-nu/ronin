# Ronin

Ronin is a fast Ninja- and Make-compatible build tool implemented in Rust.

Ronin is fully compatible with Ninja 1.14 and partially compatible with GNU
Make 4.4.1.

Known compatibility bugs and their status are recorded in
[`BUGS.md`](BUGS.md).

## Build and test

```sh
cargo build --release
cargo test --all-targets
```

The executable is `target/release/ronin`:

```sh
ronin --version
ronin -C build
ronin -t targets
```

## Make compatibility

On Unix, linking the Ronin executable as `make` makes the same binary behave
as a GNU Make-compatible tool:

```sh
ln -s ronin target/release/make
target/release/make -j8
target/release/make -f Makefile all
```

When invoked as `make` (or `gmake`), Ronin reads Makefiles and uses Make-style
command-line options. Invoking it as `ronin` or `ninja` keeps the
Ninja-compatible behavior and reads `build.ninja`.

Ronin supports GNU Make's jobserver protocol. It can join a usable inherited
jobserver and apply that shared budget to its scheduler. In Ninja mode on
Unix, a fixed `-j` budget is also published to jobserver-aware child tools,
including Cargo through `CARGO_MAKEFLAGS`. Recursive Make invocations are
already compiled into one graph and share the same scheduler and job limit,
so Ronin does not create a second jobserver for them.

## Output

Ronin's build output is Ninja's by default, so anything that parses it — an
editor, a wrapper script, a CI log scraper — sees exactly what it would see
from Ninja, on a terminal and through a pipe alike.

`--output cargo` selects a Cargo-style rendering instead: a right-aligned verb
taken from the rule's description, what it acted on, and a dimmed counter.

```
    Building CXX object CMakeFiles/libninja.dir/src/graph.cc.o (12/83)
     Linking CXX executable ninja (83/83)
    Finished 83 commands in 12.41s
```

On a terminal it also pins a progress bar to the bottom of the screen while
the build scrolls above it. Repainting is capped at thirty times a second, so
a build of very fast commands costs a handful of extra writes rather than one
per command.

`--color auto|always|never` controls escapes, and with them the bar: `auto`
emits them when stdout is a terminal and honours `NO_COLOR`, while `always`
forces them out even through a pipe. The rendering itself is never chosen by
terminal detection — only by `--output`.

Ronin's supported interface is the executable. The Rust library exists so the
binary and integration tests can share implementation; its deliberately small
embedding surface consists of `Runner`, `run`, `run_os`, `RunResult`, `Error`,
`ErrorKind`, the product/version constants, and the three process-signal
helpers re-exported at the crate root. Other modules are private and are not a
supported `ronin_core` API.

`Runner` isolates an invocation behind an explicit working directory and
caller-provided output sinks. The free `run` and `run_os` functions are
convenience wrappers that snapshot the process directory and Ninja environment
values without changing the process working directory.

## Compatibility work

Ronin's compatibility contract is in
[`docs/spec/ronin/compatibility.md`](docs/spec/ronin/compatibility.md). The
upstream Ninja test suite is the behavioral oracle for ongoing idiomatization
and performance work.

Run the complete compatibility gate with:

```sh
scripts/check-ninja-conformance.sh
```

By default the harness expects the pinned Ninja source and build trees at
`reference/ninja` and `reference/ninja-build`, which are gitignored and built
by the recipe in [`benchmarks/README.md`](benchmarks/README.md). It verifies the
source revision, accounts
for all 425 tests in 33 upstream suites using
[`tests/ninja_suite_inventory.tsv`](tests/ninja_suite_inventory.tsv), runs the
full Rust and Ninja suites, compares Ninja and Ronin tool output, and checks
bidirectional `.ninja_log` and `.ninja_deps` interoperability. Alternate paths
can be supplied with `--ninja-source`, `--ninja-build`, and `--ronin`.
