---
id [dec:ronin:make-compiles-to-ninja]
epitome "Compile Makefiles and recursive submakes into one Ninja graph; preserve Make's interface, not its executor."
state @approved
category @executive
scope {
    elements (
        [arch:ronin:make-frontend]
        [arch:ronin:graph-construction]
        [arch:ronin:execution]
        [arch:ronin:cli]
        [arch:ronin:verification]
    )
    rules (
        [spec:ronin:req:make.compiler-boundary]
        [spec:ronin:req:make.interface-compatibility]
        [spec:ronin:req:make.state-outside-the-tree+1]
        [spec:ronin:req:make.recursive-invocation+2]
        [spec:ronin:req:make.jobserver+1]
        [spec:ronin:req:make.semantics+1]
        [spec:ronin:req:make.narration]
    )
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Grow a GNU Make-compatible executor, scheduler, reporter, and recursive jobserver inside Ronin."
        rejected_because "That turns Ronin into a second Make implementation and duplicates the Ninja engine instead of compiling Make's build description into it."
    }
    {
        option "Leave recursive $(MAKE) recipes as child processes and coordinate them through GNU Make's jobserver."
        rejected_because "A nested executor fragments one build into separately scheduled graphs. Recursive Make is a graph-composition boundary and compiles as subninja instead."
    }
    {
        option "Require byte-for-byte GNU Make output, scheduling, status distinctions, and flag side effects."
        rejected_because "Those are properties of GNU Make's runner. The compatibility target is its accepted interface and the build intent represented by the compiled graph."
    }
)
consequences {
    accepted (
        "A Makefile is source code and kati is its compiler. The compiler's product is a valid Ninja-semantic graph, supplied directly in memory; execution never reparses a generated manifest."
        "Faithfulness means the compiled graph selects the same things to build and preserves targets, dependencies, ordering constraints, default goals, and recipe effects. It does not mean reproducing GNU Make's transcript or execution policy."
        "Make mode accepts the complete GNU Make 4.4.1 option vocabulary and argument shapes. Each option is either a compiler input, a mapping onto an existing Ninja execution control, or an accepted no-op. An option never justifies a Make-only branch in the build engine."
        "After compilation, the scheduler, dirtiness model, persistence, pools, depfiles, restat, console handling, failures, and narration are Ninja's. The core executor does not know that a graph came from a Makefile."
        "A recursive $(MAKE) the compiler can statically identify is `subninja`, and composing it is mandatory: kati evaluates the child invocation and composes its graph into the parent graph before execution. `subninja` names this semantic inclusion even on the direct in-memory path, where no manifest text need exist. One the compiler cannot identify is left as the shell command it is and runs, which re-enters Make mode by the invoked name and compiles another graph there; that remainder admits only what a recipe genuinely cannot settle, and every widening of what the compiler can prove shrinks it."
        "There is one Ninja scheduler for the composed graph and no nested Make executor or recursive GNU Make jobserver tree. What an uncomposable invocation starts is another compiler reading MAKEFLAGS and the environment, so no graph anywhere acquires GNU Make's scheduler, dirtiness model, or reporter. Jobserver syntax may be accepted or mapped at the outer interface without becoming an execution architecture."
        "GNU Make is consulted to determine Makefile and command-line build intent. Verification compares graphs, selected work, build outcomes, and filesystem effects; GNU Make's stdout, stderr, timing, and runner-specific ceremony are not compatibility gates."
    )
    deferred (
        "A Make construct that cannot yet be represented faithfully in the Ninja graph is a compiler gap to implement or diagnose. It is not permission to emulate Make inside the executor."
        "The exact compiler-input, Ninja-mapping, or no-op classification of every accepted flag is owned by the corrective refactor work."
    )
}
edges {
    requires (
        [dec:ronin:make-as-graph]
        [dec:ronin:multicall-identity]
    )
    supersedes ([dec:ronin:make-compatibility-oracle])
    refines ([dec:ronin:ninja-compatibility-oracle])
}
codifies (
    [spec:ronin:req:make.compiler-boundary]
    [spec:ronin:req:make.interface-compatibility]
    [spec:ronin:req:make.state-outside-the-tree+1]
    [spec:ronin:req:make.recursive-invocation+2]
    [spec:ronin:req:make.jobserver+1]
    [spec:ronin:req:make.semantics+1]
    [spec:ronin:req:make.narration]
)
affects (
    [arch:ronin:make-frontend]
    [arch:ronin:graph-construction]
    [arch:ronin:execution]
    [arch:ronin:cli]
    [arch:ronin:verification]
)
---

## Rationale

Ronin is not becoming Make. Make syntax and the Make command line are an input
language accepted by kati; the output language is the same Ninja graph Ronin
already knows how to build. This is a compiler boundary. Once the graph exists,
its provenance has no bearing on scheduling, dirtiness, persistence, process
supervision, or reporting.

Interface compatibility keeps existing build entry points usable. A caller may
invoke the make-named surface, pass GNU Make's flags and assignments, and supply
a Makefile. Supporting that vocabulary does not promise the behavior of GNU
Make's executor. Flags that affect evaluation or graph shape are compiler
inputs, controls with a natural Ninja meaning map to Ronin's existing engine,
and the remainder may be accepted without effect.

The semantic obligation is to build the same things. GNU Make remains useful as
an oracle for what targets, prerequisites, ordering, variables, and recipes a
Make invocation describes. The generated Ninja manifest remains useful as an
exact oracle for the direct graph. Neither oracle licenses Make-specific
execution code: observable files and build outcomes matter, while GNU Make's
line ordering, idle messages, recursive banners, and jobserver choreography do
not.

Recursion follows the same boundary. A recipe spelling `$(MAKE)` where the
compiler can say which invocation is meant does not ask Ronin to start a
smaller Make inside the current build. It asks the compiler to evaluate another
Make invocation and include the resulting graph as `subninja`. The child
working directory, Makefile selection, goals, variable assignments, and
graph-affecting flags are compilation inputs. The composed graph then runs
once, under the one Ninja scheduler.

Where the compiler cannot say which invocation is meant, the line runs. That is
a boundary the rule states rather than a gap it tolerates: an invocation behind
a runtime test, or inside a `.ONESHELL` recipe whose lines share a shell, is
not identifiable from the recipe at all, and refusing it turns a build GNU Make
completes into a build that never starts. What starts instead is Ronin under
its make name, compiling its own graph from MAKEFLAGS and the environment — so
the thing inside the build is another compiler, and the argument above about
Make-specific execution code is untouched by it. The remainder shrinks whenever
the compiler learns to identify more, and nothing belongs in it for being
merely awkward to lift.
