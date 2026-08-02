# Ronin

Ronin is a fast Ninja-compatible build tool implemented in Rust.

The current compatibility baseline is the Ninja build language through version
1.14. Ronin preserves Ninja-owned interfaces such as `build.ninja`,
`NINJA_STATUS`, `.ninja_log`, `.ninja_deps`, depfiles, dyndeps, pools, and
Ninja tool-mode names. `SAMUFLAGS` is intentionally unsupported; pass options
on the command line.

The current Unix package recognizes `-t browse` but does not bundle Ninja's
Python browser helper; invoking it exits with an explicit unsupported-tool
diagnostic. The remaining Linux subtools are connected.

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

Rust changes are checked with Clippy's `pedantic` and `nursery` groups in
addition to `cargo fmt`. Unsafe code is confined to the POSIX signal and GNU
Make jobserver boundaries and must document its safety invariants. Resource
ownership otherwise follows RAII; build and dependency logs expose consuming
`finish` operations where callers need to observe final flush errors.

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
`/tmp/ninja` and `/tmp/ninja-build`. It verifies the source revision, accounts
for all 425 tests in 33 upstream suites using
[`tests/ninja_suite_inventory.tsv`](tests/ninja_suite_inventory.tsv), runs the
full Rust and Ninja suites, compares Ninja and Ronin tool output, and checks
bidirectional `.ninja_log` and `.ninja_deps` interoperability. Alternate paths
can be supplied with `--ninja-source`, `--ninja-build`, and `--ronin`.
