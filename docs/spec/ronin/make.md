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

> [spec:ronin:req:make.read-interrupt]
> An interrupt during the read stops the read. Compiling a Makefile runs
> commands — a `$(shell)` call is a command line like any other — and Ronin
> MUST stop waiting for one as soon as an interruption signal arrives, whether
> it is waiting for that command's output or, the output having ended, for the
> command itself. No further command is launched afterwards, so an interrupt
> between two shell functions means the second does not run at all. The
> invocation leaves with the interrupted status
> `[spec:ronin:req:product.build-outcome]` gives it and writes nothing to
> either stream: no build was reached, and a build's narration belongs to a
> build.
>
> The command that was running is ABANDONED and MUST NOT be signalled, killed,
> or reaped. It and its own children are left running and unsignalled, to be
> reparented and buried by the init process. This is GNU Make 4.4.1's
> behaviour, measured: `fatal_error_signal` waits for the children Make runs as
> jobs and knows nothing about the one a shell function left behind, so it
> re-raises and is gone in single-digit milliseconds while that child runs on.
>
> This is the Make front end's rule and not
> `[spec:ronin:req:compat.process-integration]`'s, which governs an interrupted
> BUILD. A read has no edge to leave unreported and no changed output to
> withdraw, and where a build sends the signal Ninja sends to the same process
> groups and then stops what is still standing, a read sends nothing at all.
> The oracle is GNU Make rather than Ninja, which has no read phase to match,
> and that is what makes this a compiler-boundary rule rather than a departure
> from the Ninja contract.

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

> [spec:ronin:req:make.exported-value-charged-to-the-job]
> An exported recursive value that cannot be expanded MUST NOT refuse a run
> that starts no process carrying it. GNU Make expands such a value only in
> `target_environment`, which `start_job_command` reaches after every reason
> not to fork — the empty command, `-n`, `-q` — and which `$(shell)` reaches
> for itself; a run launching none of them never reads the value and never
> sees that it will not expand. A front end settling one environment per
> compilation unit MUST therefore hold that failure and raise it where a
> process would have been started, and MUST report it against the site the
> value was defined at, or against no site at all when it was defined in no
> file.
>
> A recipe line that comes to exactly `:` under a Bourne-compatible shell
> starts no process. `start_job_command` counts the line as started and goes
> to the next one — "People use this for timestamp rules, so avoid forking a
> useless shell" — so the target is still remade and the line is still
> reported, and no environment is built for it.
>
> How many times such a value is expanded is a separate question this does not
> answer: GNU Make expands it once per job it launches, and a compiler that
> settles one environment for a unit expands it once for the unit.

## Persistent state

> [spec:ronin:req:make.state-outside-the-tree+3]
> Make mode keeps no build state beside the build. A `make` invocation MUST
> leave neither `.ninja_log` nor `.ninja_deps` in any directory it built in,
> wherever it was invoked and whatever it built, and MUST read neither. GNU
> Make decides what is out of date from the timestamps on the disk and records
> nothing between invocations, so a Makefile-derived graph has nothing either
> file could hold that its own semantics would read back — and a directory Make
> was invoked in is one the build did not create and must leave as it found it.
>
> Kati MUST encode that timestamp-only recipe freshness with Ninja's existing
> `generator` rule control: a changed or missing persisted command hash alone
> MUST NOT make an otherwise up-to-date Make target dirty. Both the direct and
> the emitted graph carry that control, so the engine interprets it without
> consulting front-end provenance — which is what lets an emitted manifest,
> built by stock Ninja against Ninja's own state, reach the same verdict.
>
> A manifest-derived graph is unchanged: Ninja's placement, formats and names
> are Ninja's own contract. The absence is Make mode's semantics, not a
> configuration, and no front end selects an external Make namespace.

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

> [spec:ronin:req:make.question-status+1]
> Make mode's `-q` answers whether the goals are already up to date and runs
> nothing at all, and it answers in the exit status alone: zero when nothing
> would run, one when something would, two when the question could not be
> answered. This is the one place a Make invocation's status is GNU Make's
> rather than the build outcome `compatibility.md` governs, because no build is
> run to have a status of its own.
>
> An interrupt is not one of those three answers, and it overrides whichever of
> them the run had reached: a `-q` the user stops leaves with the interrupt's
> status, not with zero, one or two. The three answers are about what the
> makefile says, and two of them are affirmative — a run cut short before it
> could finish asking must not report that it finished asking, and a script
> branching on `-q` must not be told there is nothing to do by a run that never
> found out. GNU Make 4.4.1 agrees in every case measured, including one where
> it had already learned the answer was one and left 130 anyway; the two it
> leaves is for a question the makefile cannot answer — a goal with no rule, a
> makefile that will not parse, no makefile at all — and never for a signal.
> The status itself is the build outcome's 130 rather than GNU's re-raised
> signal, because the exception above is about `-q`'s own three endings and an
> interrupt is not one of them.

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

> [spec:ronin:req:make.notparallel-domain]
> `.NOTPARALLEL` with no prerequisites serialises the compilation unit that read
> it, and the unit is where the domain begins and ends. GNU Make's
> `not_parallel` is a flag of one make PROCESS — set in `snap_deps` from any
> makefile in that process's include closure, so a declaration inside an
> `include` serialises the whole of the process that included it and nothing
> else. A compiled unit is that process. Every composition of one makefile is
> therefore a domain of its own, exactly as every `$(MAKE)` GNU Make starts is a
> process of its own, and two units reading one makefile constrain each other
> not at all. What holds several compositions of one makefile apart is their
> parent, if the parent declared it.
>
> What a domain serialises is its JOBS, and a recursive recipe's job is the
> whole sub-make it stands for: `new_job` starts the job and then blocks until
> it has finished, and for a `$(MAKE)` line that is the child's entire lifetime.
> A depth-one pool answers for the unit's own command edges. It cannot answer
> for a recursion, because the compiler has dissolved that sub-make into edges
> of this same graph and a pool slot is held only while one command runs. Those
> are held apart by ordering instead: taken in a topological order of the
> unit's recursive recipes, each waits for the one before it — its wrapper and
> every edge of the children that recipe composed. The wait is on the previous
> job having finished and not on it having succeeded, so `-k` still reaches the
> recipe after a failed one, and a recipe the unit is not going to run is not a
> job and takes no place in the order.
>
> The declaration neither propagates nor restructures the graph. A
> `.NOTPARALLEL` parent hands its children `-jN` and the job budget untouched
> and their own makefiles decide their parallelism; `--shuffle` is declined for
> the declaring unit alone. And because the constraint belongs to the scheduler,
> it is wired onto a composition that has already settled — never into the
> question of whether a recipe has to run, and never into what a staging pass
> builds, both of which would turn a wait into work.
>
> `.NOTPARALLEL` WITH prerequisites is a different mechanism and not this one:
> GNU Make 4.4.1 sets a wait point between the named targets' prerequisites and
> leaves `not_parallel` clear. Only the bare form is read.

> [spec:ronin:req:make.nesting-census+2]
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
> A composition whose child directory holds no makefile is a finding rather
> than a refusal. The census names the invocation, says where it pointed and
> that nothing a Make reads is there, and carries on over the rest of the
> build — the invocations written above it, the ones written below it, and the
> children that could be read. Building the same tree is still refused: the
> child graph does not exist and the recipe line that would have started a Make
> of its own was lifted out of the recipe, so the work would simply not happen.
> The two are different judgements about the same fact, and on a tree of relic
> Makefiles the missing child is usually the most useful thing a census has to
> say — delivering it as the reason there is no census is the failure this
> distinction exists to prevent.
>
> The classification is the compiler's own. Lint reads the disposition the
> compile recorded at the moment it decided, and does not re-derive one from
> the recipe text: a census that could disagree with the build it describes
> would be describing a different build. Every other refusal — a child that
> makes its own parent's target — ends the lint where it would have ended the
> build, and is reported as the refusal it is.

> [spec:ronin:req:make.jobserver+2]
> `-j` bounds the whole Make tree and not just this process. GNU Make gives one
> jobserver per tree: the top invocation creates it, every Make below joins it
> through `--jobserver-auth` in `MAKEFLAGS`, and the tokens in it are what stop
> the levels from adding up. Ronin's budget is that budget.
>
> A parent graph and every compiled subninja share one Ninja scheduler, so a
> recursion Ronin composed costs no token beyond the slot its edge already
> holds. A recursion Ronin could not compose starts a real Make, and so does
> any jobserver-speaking tool a recipe invokes; those draw on the same pool.
> There is one pool and one scheduler, never two schedulers.
>
> Where the budget lives has three answers, and they are GNU Make's three. A
> run that joined a budget a parent published republishes the address it
> arrived under, unchanged, so a grandchild reaches the outer budget rather
> than the middle's idea of it. A run that joined none and may hand out more
> than one slot creates the budget, spends it through the same client a child
> does — the implicit slot first, tokens past it — and publishes its own
> address. A run with one slot or none to share creates and publishes nothing:
> `-j1` and a bare `-j` stand up no jobserver, as GNU Make stands up none for
> `job_slots <= 1`. Neither does a dry run — GNU Make publishes one there and
> spends it on the `+` lines it runs anyway, and Ronin runs none of them, so
> the budget would be one nothing could spend. An address this run could not
> join is not republished, and neither is one it could not create; handing a
> child a budget nobody is feeding is worse than handing it none, and a
> jobserver that cannot be made costs the budget rather than the build.
>
> `.FEATURES` claims `jobserver` and `jobserver-fifo`, and the claims are met:
> the published form is the named pipe GNU Make 4.4.1 publishes on Linux, in
> the directory `MAKE_TMPDIR` names, or `TMPDIR`, or `/tmp` — GNU Make's
> `get_tmpdir` order, and a name that does not stat as a directory is passed
> over rather than used. `--jobserver-style` is read and refused exactly where
> GNU Make refuses it, but selects nothing: the named-pipe form is the only one
> served, because a path reaches a grandchild through an intermediate process
> that passes no descriptors down.
>
> The address is settled before the makefiles are read, where GNU Make settles
> it after. Ronin compiles recipes rather than interpreting them, so each
> unit's `MAKEFLAGS` is fixed while the unit is read. Two consequences are
> accepted: a `$(MAKEFLAGS)` expanded during the read carries the address
> where GNU Make's does not yet, and a makefile's own `MAKEFLAGS += -jN`
> cannot resize a budget that already exists. What a recipe finally reads is
> the value GNU Make would have given it.

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
