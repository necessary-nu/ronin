# Ronin

Ronin is a fast Ninja-compatible build tool implemented in Rust.

The current compatibility baseline is the Ninja build language through version
1.9. Ronin preserves Ninja-owned interfaces such as `build.ninja`,
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

The original C samurai sources, Makefile, and manual page remain in the
repository as the source-port corpus. Cargo builds and tests Ronin; the legacy
C build artifacts are not part of the Ronin product interface.

## Compatibility work

Ronin's compatibility contract is in
[`docs/spec/ronin/compatibility.md`](docs/spec/ronin/compatibility.md). The
upstream Ninja test suite is the behavioral oracle for ongoing idiomatization
and performance work.
