---
id [dec:ronin:builtin-shell]
epitome "Be the shell rather than call one: where the resolved shell is the default /bin/sh, spawn this executable under that name, and leave every spelling a consumer reads untouched."
state @decided
category @executive
scope {
    elements ([arch:ronin:cli] [arch:ronin:execution] [arch:ronin:make-frontend])
    rules (
        [spec:ronin:req:product.shell-identity]
        [spec:ronin:req:product.builtin-shell]
    )
}
author "brendan@necessary.nu"
alternatives (
    {
        option "Drive an embedded nsh::Shell inside the build process, with no shell process at all."
        rejected_because "nsh's Shell reaps every child of its process — wait3(status, flags, NULL) is waitpid(-1) — so a shell running inside the supervisor would reap the compilers the scheduler is holding, and the scheduler's own wait would get ECHILD for a status sitting in a job table it cannot see. The crate's own documentation says this is not fixable by tracking pids: reaping is destructive, the peek-without-reap primitive spins, and dispatching properly would need the shell to own SIGCHLD for the whole process. A build engine cannot give that up."
    }
    {
        option "Fork the supervisor and run the shell in the child without exec'ing."
        rejected_because "fork from a multithreaded host carries only the calling thread, and the shell allocates before it execs — or never execs at all, a subshell being a shell. Same hazard as Command::pre_exec, same cause, and not removable. Ronin's supervisor is threaded."
    }
    {
        option "Parse each command into an argv in-process and run the shell only for what the parse cannot express."
        rejected_because "It re-opens the simple-versus-complex seam that needs_shell and construct_command_argv_internal already own, in a second place with a second answer, and each widening of what the parser accepts is a new chance to run a different command than the manifest asked for. Exec-self gives every line complete dash semantics with no classification at all. Weighed and put down by the operator: 'hmm, no, the original approach is the foolproof one isn't it'."
    }
    {
        option "Vendor nsh as a submodule beside kati."
        rejected_because "kati is a submodule because it is our fork and we commit into it. nsh is upstream with its own plan discipline; a fix there arrives by lock bump, not by a commit from a Ronin dispatch. Ruled by the operator: 'no, I want to use a normal git dep'."
    }
    {
        option "Write the substituted shell into the graph, so the command string names Ronin."
        rejected_because "The graph is what an emitted manifest carries, and a manifest naming Ronin's binary as its shell is not runnable by Ninja. It is also not expressible: the substitution needs argv[0] to stay `/bin/sh` while the program is this executable, and a command line has no way to say that."
    }
)
consequences {
    accepted (
        "Invoked under the name `sh`, Ronin is a shell. The name is the only door, as it is for `make` and `gmake`, so the discipline that keeps `ronin --make` in Ninja mode keeps `ronin --sh` there too."
        "A substitution spawns this executable with argv[0] set to the spelling the build named — `/bin/sh` — so the shell's own diagnostics are byte-identical to dash's, which prefixes them with argv[0] as written."
        "Debian's /bin/sh is dash, so the existing conformance, equivalence, upstream-inventory and project batteries are the differential suite for this change. A row that moves is a divergence to explain, not a number to re-record."
        "nsh is a normal git dependency pinned by Cargo.lock's rev. A divergence found here is filed upstream and arrives back as a lock bump; nothing in this repository commits into nsh."
        "The dependency is Unix-only, declared beside kati and rustix. Windows has no shell in this position at all — Ninja hands the command line to CreateProcess — so there is nothing there for a builtin shell to substitute for."
    )
    rejected (
        "That the substitution may change what a consumer reads. The graph, an emitted manifest, the dry run and the build log keep the spelling they had; only the process on the other side of the spawn is different."
        "That an explicitly named shell may be substituted. A Makefile that sets SHELL=/bin/bash gets bash. The builtin shell stands in for the default and for nothing else."
    )
}
edges {
    requires ([dec:ronin:multicall-identity])
    refines ([dec:ronin:product-boundary])
}
codifies (
    [spec:ronin:req:product.shell-identity]
    [spec:ronin:req:product.builtin-shell]
)
affects ([arch:ronin:execution])
---

## Rationale

Ronin already declines to spawn a shell for a command the shell would only
split into words, and it is right about that half: a trivial job costs
1.741 ms through `/bin/sh -c` against 0.852 ms spawned directly. The other
half — the commands that genuinely need a shell — has been an external
program on the machine, which means the interpreter for the `command` binding
is whatever `/bin/sh` happens to resolve to there: dash on Debian, bash in
POSIX mode elsewhere, busybox ash on Alpine. A build tool that compiles Make
to Ninja and then hands the result to an interpreter it does not control has
a hole in the middle of its own contract.

Owning the shell closes it. What that costs, and what it must not cost, is
the whole of this decision.

### Why the shell is a process and not a library call

nsh is a library, and the temptation is to call it. Three things in its own
documentation say not to, and each of them is fatal on its own.

A `Shell` reaps any child of the process it runs in. `wait3(status, flags,
NULL)` is `waitpid(-1)`, so a shell driven inside Ronin's supervisor would
collect the compiler the scheduler is waiting on, and the scheduler's wait
would return `ECHILD` for a status now in a job table it cannot reach. That
is not a bug to route around: reaping is destructive, the only
peek-without-reap primitive returns the same foreign child forever and turns
a blocking wait into a spin, and dispatching properly would need the shell to
own `SIGCHLD` for the whole process — which is exactly what a build engine
cannot delegate.

`fork` without `exec` from a threaded host carries only the calling thread,
and the shell allocates before it execs, or never execs at all, a subshell
being a shell. That is the same hazard `Command::pre_exec` carries and it is
not removable.

And a shell that is only *sometimes* the shell needs a rule for when, which
is the classification seam again, in a second place, with a second chance to
be wrong about what a command means.

Exec-self answers all three by construction. The shell is a process, so it
reaps its own children and nobody else's; it arrives by `exec`, so no
`fork`-without-`exec` hazard exists; and it runs every line, so there is no
seam. The cost is one `execve` of a larger binary in place of one `execve` of
a small one, on a path that was already spawning a process.

### Why argv[0] is `/bin/sh` and not `sh`

The mechanism is `Command::new(current_exe()).arg0("/bin/sh")` — a safe,
stable Unix API, no `/proc` path spelled by hand and no `unsafe`. `arg0` is
what makes this work at all: the program is Ronin's binary and the name is
the shell's, and the multicall front-end selector reads the name.

Choosing the *spelling* `/bin/sh` rather than `sh` is not cosmetic. dash
prefixes its diagnostics with `argv[0]` exactly as written, so
`sh -c nosuchcmd` says `sh: 1: nosuchcmd: not found` while `/bin/sh -c
nosuchcmd` says `/bin/sh: 1: nosuchcmd: not found`. That text is on a
failing build's stderr, which is output a consumer reads. Passing the
spelling the build named — which is the string that was about to be exec'd —
makes the substituted shell byte-identical to the one it replaced, and
generalises: substitute for `/usr/bin/sh` and the diagnostic says
`/usr/bin/sh`. Measured against dash before it was written down.

`Path::file_name` reduces `/bin/sh` to `sh`, which is what the selector
matches, so the two requirements do not conflict.

### The substitution boundary

The builtin shell stands in for the *default* shell and for nothing else.
"Default" is decidable, and both front ends already decide it:

  * kati's `simple_command::direct_argv` refuses its fast path unless
    `shell == DEFAULT_SHELL` (`b"/bin/sh"`), because a makefile-set `SHELL`
    is a statement about which interpreter runs the recipe and GNU Make
    honours it. The same test says which shell Ronin may stand in for.
  * Ninja mode's `ShellMode` resolves to `SYSTEM_SHELL` for `Auto` and
    `Compat`, and to a named program for `--shell`.

So a Makefile's `SHELL = /bin/bash`, a target-specific `SHELL`, a
command-line `SHELL=`, and `--shell /bin/zsh` all get the shell they asked
for. `--shell /bin/sh`, which asks for the default by name, gets the builtin
one, because the substitution is about the program and not about how it was
requested.

### What must not change

Everything a consumer reads. The graph's command strings, the manifest
Ronin's Make front end emits, `-n` output, and the command hash in the build
log all keep the `/bin/sh` spelling they had. The manifest requirement is the
sharp one: an emitted manifest must stay runnable by a stock Ninja, which has
no builtin shell, so the shell named in it has to be a shell that exists on
the machine. Making the substitution a spawn-time act rather than a
compile-time rewrite is what keeps all of that true at once — and it is also
why the emitted manifest and the in-process graph do not need to disagree.

### Where the verification comes from

Debian's `/bin/sh` is dash, and nsh is a port of dash whose observable
behaviour is the reference. So the battery Ronin already runs — the
conformance corpus against the GNU Make oracle, the equivalence run against
kati's own manifests, the upstream Make inventory, the Ninja conformance and
differential suites, the vim and zsh end-to-end projects — is a differential
suite for this change without being modified. A row that moves is a claim
that nsh and dash disagree, and it gets a reproducer.
