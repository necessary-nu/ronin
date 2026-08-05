# Ronin Make subsumption contract

Ronin subsumes GNU Make. A Makefile is evaluated into the same dependency
graph a Ninja manifest produces, and is then built by the same scheduler,
persistence, and process supervision. Make becomes a second front end over
Ronin's existing engine rather than a second build tool sharing a binary.

The Make front end is Ronin's fork of kati, vendored as a submodule at
`kati/`. Upstream kati converts a Makefile into a `build.ninja` file and stops;
Ronin's fork retains that emission as a debugging and verification artifact and
builds the graph directly for execution.

Make evaluation has no counterpart in Ninja and is not subject to the Ninja
compatibility contract. Where the two contracts meet — the dependency graph,
command execution, persistent state, and exit status — the Ninja contract in
`compatibility.md` governs, because a Makefile-derived graph is built by
exactly the machinery a manifest-derived graph is.

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
> The front end is selected by the invoked program name: `make` and `gmake`
> select Make mode, `ninja`, `samu`, and `ronin` select Ninja mode. The
> selection is overridable from the command line in both directions. Make mode
> identifies itself as Ronin in its own diagnostics and does not claim to be
> GNU Make except where a Makefile-visible variable requires a version string.

> [spec:ronin:req:make.question-status]
> Make mode's `-q` answers whether the goals are already up to date and runs
> nothing at all, and it answers in the exit status alone: zero when nothing
> would run, one when something would, two when the question could not be
> answered. This is the one place a Make invocation's status is GNU Make's
> rather than the build outcome `compatibility.md` governs, because no build is
> run to have a status of its own.

> [spec:ronin:req:make.recursive-invocation]
> In Make mode `MAKE` names Ronin's own executable, so recursive invocation
> re-enters Ronin rather than any other Make on the path.

> [spec:ronin:req:make.jobserver]
> Make mode participates in the GNU Make jobserver protocol as both client and
> server, including the named-pipe form, so that a recursive Make tree shares
> one job budget instead of one budget per level.

## Verification

> [spec:ronin:req:make.semantics]
> Make evaluation semantics are verified differentially against GNU Make
> 4.4.1. The pinned oracle version is a deliberate compatibility choice:
> changing it requires rerunning the corpus and reclassifying any case whose
> result moves.
