# Ronin compatibility contract

Ronin is a Ninja-compatible build tool implemented in Rust. Its product,
package, executable, and user-facing diagnostic name are **Ronin**. Ninja
remains the name of the language and compatibility surface Ronin implements.

The initial upstream oracle is Ninja `1.14.0.git` at commit
`b51a1e37c2fb89bbefa600bd155e1ce13983f09d`. Updating that pin is a deliberate
compatibility change: the upstream suite must be rerun, newly applicable tests
must be classified, and Ronin's advertised compatibility level must be
reviewed.

Compatibility means matching observable Ninja behavior on a supported
platform: manifest acceptance and evaluation, the dependency graph and
dirty-state result, commands and their environment, persistent state,
command-line and tool output, and exit status. Exact scheduling order is not
part of the contract when Ninja itself leaves a ready-edge tie unspecified;
dependency, pool, console, failure, and output-ordering guarantees are.
Platform-specific behavior may differ only when the platform cannot provide
the corresponding primitive, and the difference must be explicit and tested.

Ronin-owned names may change with the product. Ninja-owned names do not:
`build.ninja`, `.ninja_log`, `.ninja_deps`, `# ninjadeps`,
`ninja_required_version`, `ninja_dyndep_version`, `NINJA_STATUS`, manifest
bindings, and Ninja tool-mode names retain their spelling and meaning.

`SAMUFLAGS` is neither a Ninja interface nor a Ronin interface. Ronin does not
read it, does not interpret it, and does not provide a compatibility alias for
it. A replacement such as `RONINFLAGS` is not required; command-line arguments
are the supported option source.

## Product boundary

> [spec:samurai:req:product.ronin-identity]
> The distributed package, executable, help text, diagnostics, and
> documentation identify the product as `ronin` or Ronin. Ninja-owned
> compatibility identifiers retain their Ninja spelling.

> [spec:samurai:req:product.no-samuflags]
> Process startup MUST NOT read or interpret `SAMUFLAGS`. Setting or changing
> `SAMUFLAGS` has no effect on Ronin's options, selected targets, output, or
> exit status, and no legacy alias is required.

> [spec:samurai:req:compat.version-reporting]
> `ronin --version` emits one Ninja-compatible version token beginning with
> `MAJOR.MINOR` and exits successfully. The token reports the claimed Ninja
> compatibility level rather than the Cargo package version and contains no
> product-name prefix that would break consumers which parse Ninja's output.

> [spec:samurai:req:compat.ninja-owned-names]
> Ronin preserves every Ninja-owned file name, signature, manifest variable,
> environment variable, and tool-mode name used by the supported compatibility
> surface, including those enumerated above.

## Language and graph behavior

> [spec:samurai:req:compat.byte-inputs]
> Manifest paths, variable values, command data, depfile paths, and operating
> system arguments remain byte-exact where the platform permits arbitrary
> bytes. Internal processing MUST NOT silently replace invalid text or merge
> distinct byte strings through lossy Unicode conversion.

> [spec:samurai:req:compat.manifest-semantics]
> For the advertised compatibility level, Ronin matches Ninja's lexical,
> parsing, scope, expansion, include/subninja, rule, pool, default-target,
> validation, dyndep, and diagnostic acceptance behavior. A manifest requiring
> a newer unsupported Ninja version is rejected before execution.

> [spec:samurai:req:compat.graph-semantics]
> Given the same manifest and filesystem state, Ronin matches Ninja's edge and
> node relationships, target selection, dirty and ready decisions, implicit
> dependency handling, `restat`, `generator`, phony behavior, and rebuild versus
> no-op result.

## Persistent state and execution

> [spec:samurai:req:compat.persistent-state]
> Ronin reads, writes, recovers, recompacts, and invalidates `.ninja_log` and
> `.ninja_deps` compatibly with the pinned Ninja oracle, including their Ninja
> signatures and on-disk versions. A state file written by either tool is
> consumable by the other when it uses a mutually supported format.

> [spec:samurai:req:compat.scheduling]
> The scheduler enforces dependency readiness, pool depth, console-pool
> exclusivity, job and load limits, dry-run behavior, stop-after-failure
> semantics, and Ninja's observable output-ordering guarantees. It may choose a
> different ready edge only where Ninja does not specify or reliably expose the
> tie order.

> [spec:samurai:req:compat.process-integration]
> Command spawning, working directories, inherited environment, signal and
> interrupt handling, jobserver participation, child exit interpretation, and
> terminal ownership match Ninja on each supported platform.

> [spec:samurai:req:compat.command-runtime]
> Command and response-file expansion, depfile and MSVC dependency ingestion,
> response/depfile cleanup, status rendering, command echoing, buffered child
> output, and final success or failure match Ninja for supported features.

## CLI, tools, and proof

> [spec:samurai:req:compat.cli-and-tools]
> Apart from the executable name and explicitly documented exceptions, Ronin
> accepts Ninja-compatible global options, option/operand ordering, targets,
> debug and warning modes, and `-t` tools, with matching stdout, stderr, and
> exit-status behavior.

> [spec:samurai:req:compat.upstream-conformance]
> The compatibility harness pins the upstream Ninja revision and accounts for
> every upstream test: an equivalent Ronin pass, a documented platform or
> harness inapplicability, or a tracked compatibility failure. Unclassified
> exclusions and silent test omission are not permitted.

## Performance and release gates

> [spec:samurai:req:performance.reproducible-baseline]
> Performance work uses versioned, reproducible workloads covering manifest
> parsing, graph evaluation, no-op builds, dependency-log loading, and command
> scheduling. Measurements record the Ronin revision, Ninja oracle revision,
> build profile, platform, and noise-control method.

> [spec:samurai:req:performance.no-unexplained-regression]
> The optimized implementation is compared with the recorded baseline and
> pinned Ninja oracle. Material regressions in runtime, peak memory, or
> allocation count require an explanation and explicit acceptance before
> release.

> [spec:samurai:req:release.compatibility-gate]
> A Ronin release requires formatting, Rust tests, port coverage, the classified
> upstream Ninja conformance suite, and performance validation to pass for the
> release candidate, with no stale requirement annotations.
