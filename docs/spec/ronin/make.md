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

> [spec:ronin:req:make.remade-target-re-observed]
> A target whose recipe has just run is observed again on disk, and the
> timestamp it then has is what decides whether the targets that read it are
> out of date. A recipe that ran without moving its target leaves them up to
> date, as GNU Make's does. Two targets are never observed and so always count
> as remade: a phony one, which is not a file, and one the recipe left absent,
> which GNU Make reads as infinitely new. Ninja's `restat` is a narrower
> request for the same second look — it grants the outcome only to an output
> whose timestamp did not move at all — so an edge carries this property
> itself, stated when the graph is constructed, and a graph parsed from a
> manifest never carries it.

> [spec:ronin:req:make.manifest-equivalence+1]
> For any Makefile the front end accepts, the graph built directly and the
> graph obtained by parsing that front end's emitted Ninja manifest are
> equivalent: same edges, same inputs partitioned identically into explicit,
> implicit, and order-only, same outputs, same validations, same commands,
> same pool, depfile, restat, and generator bindings, and the same default
> targets.

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

> [spec:ronin:req:make.state-outside-the-tree+2]
> A Makefile-derived graph uses Ninja's build-log and dependency-log placement,
> formats, and names. Kati MUST encode GNU Make's timestamp-only recipe
> freshness with Ninja's existing `generator` rule control: a changed or
> missing persisted command hash alone MUST NOT make an otherwise up-to-date
> Make target dirty. Both the direct and emitted graphs carry that control, so
> the engine interprets it without consulting front-end provenance. Front-end
> provenance does not select an external Make namespace or suppress state
> beside the build. If state placement is configurable for Ninja graphs, that
> ordinary Ninja control is available uniformly to every front end.

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

> [spec:ronin:req:make.narration+1]
> A Makefile becomes a Ninja graph, and a Ninja graph is narrated Ninja's way.
> Make mode reports progress, failures and diagnostics in the same shape as
> the manifest front end, rather than reproducing GNU Make's wording. This is
> a product decision and not a gap: GNU Make's own test suite compares output
> byte for byte, so it is read for what a Makefile evaluated to rather than
> for how the build was announced. When an ordinary inline recipe supplies no
> description, its compiled rule uses the expanded recipe text instead of
> synthesizing a generic `build` description, so the shared reporter names the
> action that actually runs. See `make-upstream-suite`.

> [spec:ronin:req:make.recursive-invocation+2]
> A recursive invocation through `$(MAKE)` that the compiler can statically
> identify compiles as `subninja`, and it MUST: composition is not optional
> wherever the identification is possible. Kati evaluates the child Makefile
> using its requested directory, Makefile, goals, assignments, and
> graph-affecting flags, then composes the resulting graph into the parent
> graph before execution. `subninja` names this semantic graph inclusion even
> when the direct in-memory path emits no manifest text.
>
> An invocation the compiler cannot statically identify is left as the shell
> command it is, and runs. That remainder is a boundary and not a licence: it
> admits only invocations the recipe genuinely cannot settle — a multi-line
> `.ONESHELL` recipe, whose lines share one shell, so no reading of the recipe
> establishes what an earlier line left for the invocation to read; and an
> invocation reached only through a runtime test, where whether it happens at
> all is not answerable until the shell answers it. Every widening of what the
> compiler can prove shrinks the remainder, and no shape belongs in it merely
> for being hard to lift.
>
> What starts there is Ronin re-entering Make mode by its invoked name — the
> absolutized `make`-named path that `$(MAKE)` expands to — and compiling its
> own graph, with flags and any inherited job budget carried in `MAKEFLAGS` and
> the environment. The nested process is therefore another compiler and never a
> Make executor: no graph acquires GNU Make's scheduler, dirtiness model, or
> reporter by being reached this way.

> [spec:ronin:req:make.nesting-census+1]
> Linting a Makefile reports every recursive invocation the compile
> classified, each with the Makefile and line it was written on and whether it
> was composed into the graph or left to nest at run time. A composed
> invocation names the child it composed, with the `MAKE` reference written
> back in place of the path it expanded to, because the path is this process
> and says nothing a reader did not know. A long one is cut in the middle
> rather than at the end: a recipe that hands a child every path a configure
> run settled writes a thousand bytes that name one child, and the two parts
> that tell one such invocation from the next — the makefile or directory it
> selects, and the goals it asks for — sit at opposite ends of it.
>
> A nested one names the shape that kept it from composing, which is one of
> three: the recipe line does not have the invocation as its own command, so a
> shell construct stands between them; or the line's command is the invocation
> and is written as more than the argument list the resolver reads; or the
> recipe is a multi-line `.ONESHELL` whose lines share one shell. Each is
> reported beside what would compose it instead, because a reader who learns
> that a build nests and not what to change about it has learned nothing they
> can act on. Naming the shape rather than judging it is deliberate: whether a
> given shape belongs in the remainder
> `[spec:ronin:req:make.recursive-invocation]` admits is a question about the
> compiler, and a census that answered it would be arguing rather than
> reporting.
>
> A recipe line the compiler classified as recursive and could not lift is in
> the census whether or not the invocation was written where a shell would
> reach it directly, so a report shows every nested Make and not only the
> conveniently spelled ones.
>
> The classification is the compiler's own. Lint reads the disposition the
> compile recorded at the moment it decided, and does not re-derive one from
> the recipe text: a census that could disagree with the build it describes
> would be describing a different build. A refusal — a child that makes its
> own parent's target — ends the lint where it would have ended the build, and
> is reported as the refusal it is.

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

> [spec:ronin:req:make.oracle-provenance]
> The oracle is upstream GNU Make 4.4.1 as the Free Software Foundation
> released it, built from that release's own source. A distribution's build of
> 4.4.1 is a different program wearing the same version string, and where the
> two answer differently the released source decides what Ronin implements;
> the distribution's answer is a divergence to be classified and written down.
>
> Because the version string does not identify the build, a recording carries
> the identity of the Make that produced it: the reported version, the host it
> reports being built for, prose naming the source it was built from, and
> answers to a fixed set of questions on which builds of 4.4.1 are known to
> differ. Recording MUST refuse when the Make in front of it answers
> differently from that record, so that moving the oracle is an edit to the
> record and a rerun of the corpus rather than a silent overwrite. Which Make
> the recorder runs MUST be selectable, so that a second build can be measured
> against the recording without becoming it.
