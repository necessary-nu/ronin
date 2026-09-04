# Make compiler boundary audit

This audit applies the decision in
[`plan/decisions/make-compiles-to-ninja.md`](../plan/decisions/make-compiles-to-ninja.md)
to the Make implementation and its work breakdown as of 2026-08-08.

The invariant is simple: a Makefile is compiler input. Kati evaluates that
input and produces one valid Ninja-semantic graph. Ronin then plans, schedules,
persists, runs, and reports that graph exactly as it does a graph loaded from a
Ninja manifest. No Make provenance may cross that boundary.

## Classification

Every Make-specific path belongs to one of four classes:

| Class | Meaning | Disposition |
| --- | --- | --- |
| `C` | Kati/compiler semantics needed to discover faithful graph structure, bindings, commands, defaults, and exported recipe environment | Keep or fix before graph construction |
| `I` | GNU Make 4.4.1 command-line or environment interface mapped to an existing compiler input or Ninja control | Accept the spelling and argument shape, then translate once |
| `N` | Accepted interface with no faithful or useful Ninja mapping | Parse, validate when needed, and deliberately ignore |
| `R` | GNU Make runtime emulation after graph construction | Remove, retire, or replace with ordinary Ninja behavior |

`C` is compatibility of build intent. `I` and `N` are interface
compatibility. None of them authorizes a second executor.

## Landed code paths

### Compiler paths (`C`)

- `kati/src-rs/**` owns Make parsing, expansion, variable scope, implicit-rule
  search, special-target meaning, and dependency-node construction. It remains
  the language front end.
- `src/make.rs` owns the evaluation session, exported recipe environment, and
  lowering of kati dependency nodes. Its prerequisite shuffling is not
  compiler semantics and is separately classified `N` below.
- `src/make/sink.rs` lowers kati nodes, partitions inputs, bindings, pools,
  validations, defaults, and always-dirty edges into Ronin's graph.
- `src/make/equivalence.rs` compares direct lowering with kati's emitted Ninja
  manifest. It remains the graph-shape oracle.
- `src/graph.rs` support for graph properties such as an always-dirty edge is
  valid when it represents a Ninja graph idiom such as `.PHONY`.
- Front-end evaluation diagnostics may retain Makefile locations and language
  terminology. They are compiler diagnostics, not execution narration.

### Interface translation (`I`)

- `src/multicall.rs`, `src/main.rs`, and the product-name selection in
  `src/cli.rs` select the Make compiler front end when invoked through a
  Make-compatible name.
- `src/make/cli.rs` may continue to parse GNU Make 4.4.1 spellings, clustered
  short options, long-option arguments, `-C`, `-f`, `-I`, goals, and command-line
  variable assignments. It must reduce them to compiler inputs or ordinary
  Ninja execution controls before evaluation finishes.
- `-j`, `-l`, `-k`, `-n`, `-C`, and quiet/verbose controls may map to the
  corresponding single Ninja scheduler or front-end control. They do not
  create Make-specific scheduler state.
- An inherited outer jobserver may constrain the one ordinary Ninja scheduler
  through the existing generic client integration. Ronin must not serve or
  propagate a recursive Make jobserver of its own.
- `MAKE`, `MAKEFLAGS`, `MFLAGS`, `MAKEOVERRIDES`, and `MAKELEVEL` may be exposed
  to evaluation as compatibility variables. They are compiler inputs for
  recognizing and composing recursive Make intent; they are not instructions
  to launch another Make runtime.

### Accepted no-ops (`N`)

- GNU Make 4.4.1 spellings without a Ninja meaning remain accepted with their
  documented argument shapes. Current examples include `--debug`, `-d`,
  `--trace`, `--shuffle`, `-t`, `-W`/`--what-if`, and output synchronization
  modes. The complete table belongs to `make-interface-surface`.
- `src/make.rs` prerequisite/goal reordering for `--shuffle` is removed; the
  option is accepted without perturbing the compiled graph.
- `-t` and `-W` do not mutate Ronin's dirtiness model or timestamps. `-O` does
  not install a Make output grouper. Debug flags do not install a Make
  narrator.
- Make-only jobserver authorization flags are accepted and ignored when they
  cannot be translated to the one scheduler safely.

### Runtime emulation to remove (`R`)

- `src/make/cli.rs`: recursive-process `MAKEFLAGS` propagation, `MAKELEVEL`
  process depth, jobserver publication, Make default serialism, Make directory
  announcements, output grouping, debug narration, timestamp pretending,
  intermediate-file cleanup after execution, question/touch execution forks,
  and Make-specific completion/status handling.
- `src/make/report.rs`: `Stop.` suffixes, Make idle text, recursive program
  prefixes, Make recipe-failure lines, keep-going summaries, and GNU-specific
  exit narration. Retain only translation of front-end evaluation failures into
  ordinary Ronin diagnostics.
- `src/cli.rs`: Make-only directory banners and any Make-specific setup of the
  runner, served jobserver, or recursive environment. Generic Ninja controls
  and optional outer-jobserver consumption remain provenance-free.
- `src/build.rs`: `serve_jobserver`, recursive `MAKEFLAGS` publication,
  `MAKELEVEL`, `recipe_failure`, Make output grouping, and Make-specific
  scheduling branches.
- `src/jobserver.rs`: the served FIFO/pipe transport, token publication, and
  recursive-tree environment. A generic inherited-client adapter may remain if
  it constrains every frontend identically.
- `src/build/command.rs`: Make-only closing/failure handling. Touch behaviour is
  no longer removed — see the `-t` note under "Accept without emulation".
- `src/build/reporter.rs`: Make recipe-failure rendering. A Makefile-derived
  graph uses the ordinary Ninja reporter.
- `src/frontend/execute.rs`: the Make-specific external persistence namespace
  and any Make-only state/log selection. Equivalent graphs share the same
  persistence policy.
- `src/graph.rs`: Make-only timestamp exceptions and `-W` assumed-new state.
  Dirtiness is the ordinary Ninja graph/log calculation.
- `examples/make_conformance.rs` and `scripts/check-make-conformance.sh`: exact
  stdout/stderr and GNU runner-status comparison. Rewrite the gate around graph
  shape, selected work, normal outcome, and filesystem effects.
- Make-wording expectations in `examples/make_upstream.rs`, `tests/make/**`,
  `tests/make_port.rs`, and `tests/make_state.rs` become discovery evidence or
  build-intent assertions; they are not execution-parity gates.

## WBS disposition

The existing WBS contains useful compiler work mixed with tasks that attempted
to turn Ronin into GNU Make. The following disposition is authoritative for the
remaining work.

### Keep as compiler work (`C`)

- Foundations already delivered: `kati-submodule`, `ronin-frontend-api`,
  `kati-global-state` and all of its children, `kati-build-sink`,
  `kati-graph-sink`, `make-graph-equivalence`, `make-licence-position`,
  `ronin-execute-api`, `kati-quality-gates`, `kati-command-escapes`,
  `make-phony-always-dirty`, and `ronin-package-with-fork`.
- Make-language and lowering work: `make-parser-parity`,
  `make-deferred-output-portability`, `make-features-variable`,
  `make-recipe-one-shell-per-line`, `make-vpath-search`,
  `make-suffixes-with-prerequisites`, `make-implicit-rule-chaining`,
  `make-dot-prefixed-target-nodes`, `make-second-expansion`,
  `make-special-target-noise`, `make-default-target-rule`,
  `make-order-only-variable`, `make-wait-prerequisite`,
  `make-prerequisite-globbing`, `make-target-variable-order`,
  `make-intermediate-files`, `make-second-expansion-patterns`,
  `make-ignore-errors-target`, `make-export-to-recipes`,
  `make-notparallel-target`, `make-private-variables`,
  `make-ignore-errors-status`, `make-include-semantics`, and
  `make-assignment-modifiers`.
- Verification, limited to build intent: `make-conformance-corpus`,
  `make-upstream-suite`, `make-ported-correctness-suite` and its
  `make-corpus-features`, `make-corpus-functions`, and `make-corpus-targets`
  children, plus `make-real-project-gate`.

The verification nodes must compare evaluation and graph/build effects, not
GNU Make's scheduler, timing, wording, or private statuses.

### Keep as interface translation (`I`)

- `make-mode-cli` keeps product selection and the Make-compatible front door;
  recursive execution moves to `make-subninja-recursion`.
- `make-option-parity`, `make-option-negations`,
  `make-option-include-path`, `make-option-no-builtin-variables`,
  `make-option-eval-and-warnings`, `make-recipe-dry-run`, and
  `make-switch-precedence` become input classification or translations to
  compiler/Ninja controls.
- `make-variable-is-one-word` and `make-option-carrying` are recast as
  compiler-visible invocation data used to recognize and compose subninja
  inputs; they do not propagate a recursive runtime.
- `make-refusal-reaches-further` retains only correct compiler rejection and
  source location. GNU Make wording is discarded.

### Accept without emulation (`N`)

- `make-option-output-sync`, `make-option-debug-and-trace`,
  `make-option-shuffle`, and the `-W` half of `make-option-touch-and-what-if`
  collapse into the complete interface table owned by
  `make-interface-surface`. Their spellings are accepted; their Make runtime
  behavior is not implemented.

- **`-t` is the exception, and it is one by operator decision** (Brendan,
  2026-08-17): touch mode is implemented rather than accepted. The reason the
  original disposition does not hold is that `-t` is not a reporting or
  scheduling behaviour — it decides what the run writes to disk, and
  `[spec:ronin:req:make.semantics+1]` makes filesystem effects a conformance
  criterion. A `-t` that runs the recipes is not an unimplemented no-op; it is
  the opposite of what was asked for, and it overwrites the files the caller
  told it not to make.

  What is implemented is the behaviour and not the voice. Each edge that would
  have run has its outputs dated instead, `.PHONY` targets are declined, `-n`
  keeps its precedence, `-q` still answers without running, `MAKEFLAGS` carries
  `t` to a recursive child, and a target written `lib.a(member.o)` is dated by
  writing the archive's own mtime into that member's index entry — GNU Make's
  `ar_member_touch`, which is the only path by which a date ever reaches a
  member of an archive `ar` wrote in its default deterministic mode. GNU Make's
  `touch <file>` line is NOT reproduced: the touched edge is reported by the
  ordinary Ninja progress line under `[spec:ronin:req:make.narration+2]`, and a
  second line naming the same work in GNU Make's words would be narration
  rather than information.

### Retire or reverse (`R`)

- Unshipped behavior-only tasks: `make-jobserver-server`,
  `make-diagnostic-stop`, `make-abandon-exit-status`,
  `make-directory-announce-level`, `make-nothing-to-be-done`, and
  `make-keep-going-report`.
- Wording-only work: `make-refusal-wording-is-narration`,
  `make-io-error-wording`, and the GNU-wording half of
  `make-diagnostic-prefix` and `make-fatal-diagnostic-shape`. Compiler
  diagnostic identity remains valid through `make-diagnostic-identity`.
- Landed execution emulation to reverse: `make-state-outside-the-tree`,
  `make-recursive-jobserver-reach`, `make-jobserver-explicit-under-parent`,
  `make-timestamp-dirtiness`, and `make-recipe-failure-line`.
- `make-option-coverage` is historical evidence only. Every accepted spelling
  is reclassified under the complete interface table.

#### Correction, 2026-08-31: the jobserver is not runtime emulation

The three jobserver rows above — `make-jobserver-server`,
`make-recursive-jobserver-reach`, `make-jobserver-explicit-under-parent` —
were classified `R` on the reading that a token server is a second executor.
It is not one. `-j` is a claim on the machine and not on this process, and the
jobserver is the only protocol that makes the claim hold across processes: it
is what `cargo`, `cc -flto=jobserver` and Ninja itself already join, and what
Ronin's manifest mode has always served. Ronin's Make mode measured three
recipes at once under `-j2` because of the misclassification, in every
direction that involved a real child process.

What the boundary actually forbids is a second *scheduler*, and one budget
spent through one client by one Ninja scheduler is the opposite of that: see
`[spec:ronin:req:make.jobserver+3]`. Recursions Ronin composes still cost no
token beyond the slot their edge already holds. Reclassified `I`.

### Refactor chain

The replacement leaves own the boundary in dependency order:

1. `make-makelike-audit` records this disposition and corrects the WBS.
2. `make-interface-surface` owns the complete GNU Make 4.4.1 spelling/argument
   matrix and its `C`/`I`/`N` classification.
3. `make-subninja-recursion` recognizes recursive `$(MAKE)` intent and composes
   the referenced Makefile as a subninja graph instead of launching a submake.
4. `make-single-ninja-scheduler` removes served/recursive jobservers and leaves
   one scheduler for the composed graph.
5. `make-ninja-narration` deletes GNU Make execution ceremony.
6. `make-executor-boundary` removes all remaining Make provenance from
   dirtiness, persistence, planning, supervision, and reporting.
7. `make-build-intent-gate` replaces runner-parity gates with graph and
   filesystem-effect gates.

Completion means that deleting the frontend provenance after graph construction
cannot change how the graph is planned or run.
