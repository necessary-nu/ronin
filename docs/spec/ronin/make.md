# Ronin Make subsumption contract

Ronin subsumes the Make interface by compiling it, not by becoming GNU Make.
A Makefile is source code, kati is its compiler, and a valid Ninja-semantic
graph is the compiler output. That graph is built by the same scheduler,
persistence, and process supervision as a parsed Ninja manifest. Make is a
second front end over Ronin's existing engine, never a second build tool
sharing the binary.

The Make front end is Ronin's fork of kati, vendored as a submodule at
`kati/`. Upstream kati converts a Makefile into a `build.ninja` file and stops;
Ronin's fork retains that emission as a debugging and verification artifact and
builds the graph directly for execution.

Make evaluation has no counterpart in Ninja, so GNU Make remains the oracle
for the build intent the source language expresses. At the compilation
boundary and beyond — graph construction, command execution, persistent state,
and build outcome — the Ninja contract in `compatibility.md` governs, because
a Makefile-derived graph is built by exactly the machinery a
manifest-derived graph is.

## Graph construction

> [spec:ronin:req:frontend.graph-construction]
> Graph construction is a front-end-agnostic capability. Building a graph and
> parsing a Ninja manifest are separately invocable: a front end supplies
> rules, pools, edges, and default targets, and receives a graph the build,
> tool, and persistence paths accept without knowing which front end produced
> it.

> [spec:ronin:req:make.graph-direct]
> Make mode constructs the dependency graph in memory. No serialized manifest
> is written, read, or reparsed on the path from Makefile to execution.

> [spec:ronin:req:make.compiler-boundary]
> Kati compiles a Makefile invocation into a valid Ninja-semantic graph. Every
> Make construct that affects the build is represented by graph structure,
> edge command text, or another existing Ninja execution control before the
> graph reaches the engine. The scheduler, dirtiness model, persistence,
> process supervision, and reporter contain no Make-specific execution
> semantics.

> [spec:ronin:req:make.phony-always-dirty]
> A `.PHONY` target is never up to date. The edge it produces is out of date
> whenever it is reached, whatever its outputs' timestamps are and whatever the
> build log recorded for them, so its recipe runs on every build that asks for
> it, as GNU Make's does. An edge carries the property itself, stated when the
> graph is constructed. A Ninja manifest has no syntax for it and needs none —
> Ninja's own way to say it is a dependency on a path nothing produces — so a
> graph parsed from a manifest never carries it.

> [spec:ronin:req:make.manifest-equivalence]
> For any Makefile the front end accepts, the graph built directly and the
> graph obtained by parsing that front end's emitted Ninja manifest are
> equivalent: same edges, same inputs partitioned identically into explicit,
> implicit, and order-only, same outputs, same validations, same commands,
> same pool and depfile and restat bindings, and the same default targets.

## Evaluation state

> [spec:ronin:req:make.no-ambient-state]
> The Make front end holds no mutable process-global state. Symbol interning,
> variable scope, file and glob and find caches, command results, evaluation
> statistics, and command-line flags are owned by an explicit session value
> passed through evaluation. Two sessions may be evaluated in the same process
> without either observing the other.

> [spec:ronin:req:make.scope-separation]
> Symbol interning and Make's global variable scope are distinct. Interning a
> name does not create, read, or modify a variable binding, and a session's
> variable scope can be replaced without reinterning its symbols.

## Persistent state

> [spec:ronin:req:make.state-outside-the-tree]
> A Make-mode build writes nothing of its own into the tree it builds: the
> directory holds exactly what the Makefile's recipes put there, as it does
> after a GNU Make build. The state that makes the next build incremental is
> relocated rather than discarded, because the build log is what notices a
> changed command and the dependency log is what carries a compiler's reported
> headers to the next build, and GNU Make can do neither. Both keep Ninja's
> formats and Ninja's names, in a per-tree entry under Ronin's state home:
> `$RONIN_STATE_HOME` when it names an absolute path, otherwise
> `$XDG_CACHE_HOME/ronin`, otherwise the platform's per-user cache directory,
> which is `$HOME/.cache/ronin` and `$HOME/Library/Caches/ronin` on macOS. A
> build with none of those to work from is refused, naming them, rather than
> falling back into the tree.
>
> The entry is keyed by the identity of the directory the build runs in, which
> is that directory's resolved absolute path together with its inode: two
> checkouts of one project never share an entry, and a tree that was moved or
> replaced since it was last built starts from nothing rather than inheriting
> what the directory recorded before. Losing usable state is the preferred
> failure, because a build that rebuilds more than it had to is slow and one
> that rebuilds less is wrong. Each entry names the tree it belongs to, and an
> entry claimed by a different tree is refused rather than read.
>
> Ninja mode is unaffected. A manifest's `.ninja_log` and `.ninja_deps` stay in
> its build directory, where `compat.persistent-state` requires them.

## Product surface

> [spec:ronin:req:product.make-identity]
> The front end is selected by the invoked program name and by nothing else.
> The whole name must be `make` or `gmake` for Make mode; every other name,
> Ronin's own included, selects Ninja mode. No command-line option selects a
> front end, so `MAKE` is a path rather than a command line. Make mode
> identifies itself as Ronin in its own diagnostics and does not claim to be
> GNU Make except where a Makefile-visible variable requires a version string.

> [spec:ronin:req:make.interface-compatibility]
> Make mode accepts every option spelling and argument shape exposed by GNU
> Make 4.4.1, including options received through `MAKEFLAGS`. Acceptance is an
> interface guarantee, not a promise to reproduce GNU Make's runner. Each
> option is classified as a kati compiler input, a mapping to an existing
> Ninja execution control, or an accepted no-op; no option introduces a
> Make-only scheduler, persistence, jobserver, or reporting path.

> [spec:ronin:req:make.question-status]
> Make mode's `-q` answers whether the goals are already up to date and runs
> nothing at all, and it answers in the exit status alone: zero when nothing
> would run, one when something would, two when the question could not be
> answered. This is the one place a Make invocation's status is GNU Make's
> rather than the build outcome `compatibility.md` governs, because no build is
> run to have a status of its own.

> [spec:ronin:req:make.narration]
> A Makefile becomes a Ninja graph, and a Ninja graph is narrated Ninja's way.
> Make mode reports progress, failures and diagnostics in the same shape as
> the manifest front end, rather than reproducing GNU Make's wording. This is
> a product decision and not a gap: GNU Make's own test suite compares output
> byte for byte, so it is read for what a Makefile evaluated to rather than
> for how the build was announced. See `make-upstream-suite`.

> [spec:ronin:req:make.recursive-invocation+1]
> A recursive invocation through `$(MAKE)` compiles as `subninja`. Kati
> evaluates the child Makefile using its requested directory, Makefile, goals,
> assignments, and graph-affecting flags, then composes the resulting graph
> into the parent graph before execution. `subninja` names this semantic graph
> inclusion even when the direct in-memory path emits no manifest text. It is
> not a nested Make process or executor.

> [spec:ronin:req:make.jobserver+1]
> Ronin does not create a GNU Make jobserver for recursive Make execution. A
> parent graph and every compiled subninja share one Ninja scheduler and one
> job limit. Jobserver-related option spellings and authentication tokens are
> accepted at the Make interface; the outer invocation may map a usable
> inherited budget onto the Ninja scheduler or ignore it, but it does not
> introduce a second scheduling mechanism.

## Verification

> [spec:ronin:req:make.semantics+1]
> GNU Make 4.4.1 is the oracle for the build intent expressed by Makefile
> evaluation and the accepted command-line interface. Verification compares
> the compiled graph, selected work, normal build outcome, and filesystem
> effects. GNU Make's stdout, stderr, scheduling, idle and diagnostic wording,
> recursive banners, jobserver choreography, and runner-specific status
> distinctions are not conformance criteria. Moving the pin requires rerunning
> the corpus and reclassifying changes in build intent.
