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

The current Unix distribution does not bundle Ninja's Python `browse` helper.
Ronin recognizes `-t browse` but reports it as unsupported instead of starting
the browser frontend. All other Linux tools listed by the pinned oracle are in
the supported CLI surface.

## Product boundary

> [spec:ronin:req:product.ronin-identity]
> The distributed package, executable, help text, diagnostics, and
> documentation identify the product as `ronin` or Ronin. Ninja-owned
> compatibility identifiers retain their Ninja spelling.

> [spec:ronin:req:product.no-samuflags]
> Process startup MUST NOT read or interpret `SAMUFLAGS`. Setting or changing
> `SAMUFLAGS` has no effect on Ronin's options, selected targets, output, or
> exit status, and no legacy alias is required.

> [spec:ronin:req:product.output-style]
> Ronin's build output is Ninja's unless another rendering is named by an
> explicit command-line option. Terminal detection, `NINJA_STATUS`, and other
> environment inspection never change which rendering is selected, so a
> consumer that parses Ronin's output sees the same bytes whether or not it is
> attached to a terminal. Colour is a separate decision from rendering: it is
> emitted only when the destination is a terminal, suppressed when `NO_COLOR`
> is set to a non-empty value, and forced on or off by `--color`.

> [spec:ronin:req:product.command-execution]
> On Unix, Ninja passes each `command` to `/bin/sh -c`, which makes the shell
> the interpreter for the binding — its quoting rules, operators, and
> `VAR=value` prefixes. Ronin preserves that meaning exactly. It does not
> always preserve the shell process: a command the shell would do nothing to
> but split into words and execute may be executed directly. A command whose
> program cannot be resolved is handed to the shell regardless, so the
> diagnostic and exit status are the shell's own rather than an imitation of
> them. `--compat` spawns a shell for every command, as Ninja does, and
> `--shell` selects which shell that is. On Windows there is no shell in this
> position at all: Ninja hands the whole command line to `CreateProcess` and
> lets Windows find the program in it, so `--compat` asks for what already
> happens and POSIX word splitting is not applied. `--shell` still selects one,
> which is how a shell that runs on Windows is used there.

> [spec:ronin:req:product.shell-identity]
> Invoked under the name `sh`, Ronin is a POSIX shell rather than a build
> tool: it reads the argument vector a dash-compatible shell reads and answers
> as one. The name is the only way in, exactly as it is for the Make front
> end — no option selects it, and nothing about a build reaches it. `argv[0]`
> is reported as it was written, so a diagnostic names the shell its caller
> named rather than the file that answered.

> [spec:ronin:req:product.builtin-shell]
> Where a command needs a shell and the shell resolved for it is the default
> `/bin/sh`, Ronin runs that shell itself, by spawning its own executable
> under the name the build asked for. The substitution is a spawn-time act
> and changes nothing a consumer reads: the graph, an emitted manifest, the
> dry run and the build log carry the spelling they carried before, so a
> manifest Ronin writes stays runnable by a build tool that has no shell of
> its own. A shell the build names — a Makefile's `SHELL`, a target-specific
> `SHELL`, a command-line `SHELL=`, or `--shell` — is spawned as named, so
> choosing a shell still chooses one.

> [spec:ronin:req:product.build-outcome+1]
> A build that does not finish reports why on stdout, after the build's own
> output, and leaves with the exit status of the last command that failed — the
> command's own status, not a generic failure, so a caller can tell a compile
> error from a kill.
>
> A run cut short by a signal delivered to Ronin itself ends differently, and
> the ending is the signal. The running commands are stopped, what they were
> making is withdrawn, and the process then dies of the signal it caught, so a
> shell reads 128 + the signal number: 130 for `SIGINT`, 143 for `SIGTERM`, 129
> for `SIGHUP`. `SIGQUIT` is the one exception and leaves 1 without re-raising,
> because its default action writes a core file and a build the user quit is not
> a fault. This is GNU Make's disposition exactly, it governs BOTH front ends,
> and in the manifest front end it is a deliberate departure from upstream
> Ninja, which returns a fixed `ExitInterrupted` of 130 for every signal it
> treats as an interrupt and never re-raises. The 130 itself is kept wherever a
> status is a value rather than an ending — what a library caller's outcome
> carries, and what tells a stopped build from a failed one — and only the
> ending moved.
>
> Two further departures from Ninja are deliberate. A plan that can make no
> progress without having recorded a failure — which Ninja calls a bug in
> itself — leaves with a failure rather than Ninja's success, because a build
> that did not finish must not report that it did. Ninja's arithmetic for a
> CHILD killed by a signal it does not treat as an interrupt adds 128 to the raw
> wait status rather than to the signal number, which is reproduced, including
> the resulting `FAILED: [code=259]` for a dumping `SIGQUIT`, because that line
> is part of the observable output.

> [spec:ronin:req:compat.version-reporting]
> `ronin --version` emits one Ninja-compatible version token beginning with
> `MAJOR.MINOR` and exits successfully. The token reports the claimed Ninja
> compatibility level rather than the Cargo package version and contains no
> product-name prefix that would break consumers which parse Ninja's output.

> [spec:ronin:req:compat.ninja-owned-names]
> Ronin preserves every Ninja-owned file name, signature, manifest variable,
> environment variable, and tool-mode name used by the supported compatibility
> surface, including those enumerated above.

## Language and graph behavior

> [spec:ronin:req:compat.byte-inputs]
> Manifest paths, variable values, command data, depfile paths, and operating
> system arguments remain byte-exact where the platform permits arbitrary
> bytes. Internal processing MUST NOT silently replace invalid text or merge
> distinct byte strings through lossy Unicode conversion.

> [spec:ronin:req:compat.manifest-semantics]
> For the advertised compatibility level, Ronin matches Ninja's lexical,
> parsing, scope, expansion, include/subninja, rule, pool, default-target,
> validation, dyndep, and diagnostic acceptance behavior. A manifest requiring
> a newer unsupported Ninja version is rejected before execution.

> [spec:ronin:req:compat.graph-semantics]
> Given the same manifest and filesystem state, Ronin matches Ninja's edge and
> node relationships, target selection, dirty and ready decisions, implicit
> dependency handling, `restat`, `generator`, phony behavior, and rebuild versus
> no-op result.

## Persistent state and execution

> [spec:ronin:req:compat.persistent-state]
> Ronin reads, writes, recovers, recompacts, and invalidates `.ninja_log` and
> `.ninja_deps` compatibly with the pinned Ninja oracle, including their Ninja
> signatures and on-disk versions. A state file written by either tool is
> consumable by the other when it uses a mutually supported format.

> [spec:ronin:req:compat.scheduling]
> The scheduler enforces dependency readiness, pool depth, console-pool
> exclusivity, job and load limits, dry-run behavior, stop-after-failure
> semantics, and Ninja's observable output-ordering guarantees. It may choose a
> different ready edge only where Ninja does not specify or reliably expose the
> tie order.

> [spec:ronin:req:compat.process-integration+2]
> Command spawning, working directories, inherited environment, signal and
> interrupt handling, jobserver participation, child exit interpretation, and
> terminal ownership match Ninja on each supported platform.
>
> What an interrupt does is Ninja's throughout. No further command is launched,
> including the next command line of a recipe the front end gave several of; no
> edge that was running is reported as finished or recorded; and every output
> such an edge had already changed is withdrawn — which is what Ninja's
> `Builder::Cleanup` does to its active edges, whatever status the command it
> killed eventually reported.
>
> One departure is deliberate, and it is about how long that takes rather than
> about what it is. Ninja signals each running command's process group and then
> blocks in `waitpid` for every one of them with no bound, so a command that
> declines the signal — or whose shell took it between two of the command lines
> it was given and carried on to the next — holds the build until it finishes
> of its own accord, and its side effects up to that point stand. Ronin sends
> the same signal to the same groups and gives them the same chance to take it,
> and then stops what is still standing rather than being held by it.

> [spec:ronin:req:compat.command-runtime]
> Command and response-file expansion, depfile and MSVC dependency ingestion,
> response/depfile cleanup, status rendering, command echoing, buffered child
> output, and final success or failure match Ninja for supported features.

## CLI, tools, and proof

> [spec:ronin:req:compat.cli-and-tools]
> Apart from the executable name and explicitly documented exceptions, Ronin
> accepts Ninja-compatible global options, option/operand ordering, targets,
> debug and warning modes, and `-t` tools, with matching stdout, stderr, and
> exit-status behavior.

> [spec:ronin:req:compat.upstream-conformance]
> The compatibility harness pins the upstream Ninja revision and accounts for
> every upstream test: an equivalent Ronin pass, a documented platform or
> harness inapplicability, or a tracked compatibility failure. Unclassified
> exclusions and silent test omission are not permitted.

> [spec:ronin:req:tools.lint]
> `-t lint` reports what compiling a build already established about it, and
> builds nothing. It is Ronin's own tool rather than a Ninja-owned name — the
> first entry Ronin adds to the `-t` set — and it is listed by `-t list`
> beside the tools Ninja owns; no Ninja-owned tool name changes meaning to
> make room for it, and no operand acquires a second reading, which is why the
> surface is a tool and not a subcommand: in Ninja mode an operand is a target.
>
> The input is the file the invocation names — `-f`, defaulting to Ninja's
> `build.ninja` — and its kind is read from that name. `GNUmakefile`,
> `makefile`, `Makefile`, or a `.mk` suffix is a Makefile; every other name is
> a Ninja manifest; `--make` and `--ninja` name the kind outright for a file
> spelled otherwise. Reading a Makefile here is not selecting a front end:
> `[spec:ronin:req:product.make-identity]` governs which front end builds, and
> lint runs no build for a front end to be selected for.
>
> Reading is the read phase a build would perform, and nothing less. A
> Makefile is evaluated: `$(shell)` runs, `$(warning)` and `$(info)` print,
> and a makefile the read must remake is remade, because GNU Make's read phase
> does all three and a report about a quieter read would be a report about a
> different build. Lint states that in its own help rather than implying a
> hermetic read it does not perform.
>
> Findings are compiler diagnostics in Ronin's established shapes: `<loc>:
> <message>` for a located finding, `<loc>: warning: <message>` for one that
> names a problem, `<loc>: note: <message>` for the line that says what would
> answer it, and `ronin: <message>` for a finding with no location and for the
> closing summary. Nothing is rendered in GNU Make's voice. The exit status is
> the worst finding: zero when nothing above a note was found, one when a
> warning was, and two when an error was, or the input could not be read.

> [spec:ronin:req:tools.manifest-lint]
> Linting a Ninja manifest reports what parsing accepts and building would not
> question: a build statement's binding that no rule it can reach ever reads,
> a dependency cycle anywhere in the graph rather than only under the targets
> a build happened to be asked for, and a phony statement carrying bindings
> that can never run or a rule named `phony` that shadows the built-in one
> without being it. What the parser already refuses — a duplicate output, an
> unknown rule, an unexpected rule variable — is reported as the parse error
> it is, in lint's shape, so one command answers for every static failure a
> manifest can have. Lint changes nothing about what the parser accepts: a
> manifest that lints with findings builds exactly as it did before.

## Performance and release gates

> [spec:ronin:req:performance.reproducible-baseline]
> Performance work uses versioned, reproducible workloads covering manifest
> parsing, graph evaluation, no-op builds, dependency-log loading, and command
> scheduling. Measurements record the Ronin revision, Ninja oracle revision,
> build profile, platform, and noise-control method.

> [spec:ronin:req:performance.no-unexplained-regression]
> The optimized implementation is compared with the recorded baseline and
> pinned Ninja oracle. Material regressions in runtime, peak memory, or
> allocation count require an explanation and explicit acceptance before
> release.

> [spec:ronin:req:performance.allocation-accounting]
> Allocation-sensitive optimization work is measured by a deterministic
> in-process harness that counts allocator requests and requested bytes for
> each versioned baseline workload and reports them per build statement.
> Recorded allocation baselines are machine-readable, carry schema, workload
> version, and build-profile provenance, and a check mode fails when a
> measured workload exceeds its recorded values by more than an explicit
> tolerance.

> [spec:ronin:req:performance.make-oracle-baseline]
> Make mode's wall time is recorded against the same GNU Make 4.4.1 oracle the
> conformance and equivalence gates use, on the same trees, the same targets
> and the same `-j`, with the two tools sampled interleaved rather than one
> after the other. The workloads separate what a single real project mixes
> together: a wide graph in one directory with no recursion, a deep tree of
> directories whose Makefiles hold almost no graph, the two hand-written real
> build systems at their up-to-date and incremental steady states, and a clean
> build from empty. Each workload asserts the shape it claims — a no-op that
> builds something, or an incremental run that builds nothing, is refused
> rather than timed — and the gate refuses to measure at all on a host whose
> load average says the number would be noise.
>
> The recorded baseline is what was measured, including where Ronin is slower
> than GNU Make, and validation refuses a Ronin/GNU ratio materially worse than
> the recorded one. There is no absolute threshold in either direction: the
> gate's subject is the direction of travel, and a threshold Ronin does not
> meet today would be a gate nobody could leave switched on.

> [spec:ronin:req:release.compatibility-gate]
> A Ronin release requires formatting, Rust tests, port coverage, the classified
> upstream Ninja conformance suite, and performance validation to pass for the
> release candidate, with no stale requirement annotations.
