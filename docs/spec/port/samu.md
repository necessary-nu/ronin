# samu.c

> [spec:ronin:def:samu.debugflag-fn]
> static void debugflag(const char *flag)

> [spec:ronin:sem:samu.debugflag-fn]
> Compares `flag` with the supported debug switches. `explain` enables
> build-explanation output, `keepdepfile` preserves depfiles, and `keeprsp`
> preserves response files. Any other value is a fatal configuration error.

> [spec:ronin:def:samu.getbuilddir-fn]
> static char * getbuilddir(void)

> [spec:ronin:sem:samu.getbuilddir-fn]
> Looks up `builddir` in the root build environment. If it is absent, returns
> null. Otherwise creates that directory hierarchy without treating an
> existing directory as an error; exits unsuccessfully if that operation
> fails, and returns the environment string's stored path without copying it.

> [spec:ronin:def:samu.jobsflag-fn]
> static void jobsflag(const char *flag)

> [spec:ronin:sem:samu.jobsflag-fn]
> Parses the complete argument as a base-10 signed integer. A trailing
> non-numeric character or a negative value is fatal. A positive value becomes
> the maximum job count; zero selects the sentinel `-1` meaning unlimited
> jobs.

> [spec:ronin:def:samu.loadflag-fn]
> static void loadflag(const char *flag)

> [spec:ronin:sem:samu.loadflag-fn]
> On platforms with load-average support, parses the entire argument as a
> non-negative floating-point value and stores it as the scheduler's maximum
> load. Conversion errors, trailing characters, and negative values are
> fatal. On other platforms it leaves scheduling unchanged and emits a warning
> that the option is unsupported.

> [spec:ronin:def:samu.main-fn+1]
> int main(int argc, char *argv[])

> [spec:ronin:sem:samu.main-fn+1]
> Derives the Ronin program name and parses command-line options for directory,
> manifest, limits, debug and warning switches, verbosity, dry-run mode,
> version output, and a selected Ninja tool. It deliberately does not read
> `SAMUFLAGS`. Invalid or incomplete options print usage and exit. It chooses a
> default job count from the processor count (2 for at most one processor, 3
> for two, otherwise CPUs plus 2), obtains `NINJA_STATUS` or the default status
> format, and line-buffers standard output. For each manifest attempt it
> reinitializes graph, environment, and parser state, then parses the manifest.
> A selected tool runs immediately with the remaining positional arguments.
> Otherwise it opens the build and dependency logs, rebuilds a generated
> manifest when it is dirty, retrying parsing up to 100 times after a real
> (non-dry-run) manifest rebuild that dirties its output or prunes
> dependencies. It resets build state after that special build. Finally it adds
> each requested target (failing for an unknown target), or all declared
> default targets, performs the build, closes both logs, and returns success.

> [spec:ronin:def:samu.parseenvargs-fn+1]
> Ronin has no environment-option parsing helper; its CLI parser accepts only
> the process argument vector.

> [spec:ronin:sem:samu.parseenvargs-fn+1]
> Ronin does not read or interpret `SAMUFLAGS` and exposes no replacement
> environment-option parser. All supported CLI options come from the process
> argument vector.

> [spec:ronin:def:samu.progname-fn]
> static const char * progname(const char *arg, const char *def)

> [spec:ronin:sem:samu.progname-fn]
> Returns `def` when `arg` is null; otherwise returns the substring following
> the final `/` in `arg`, or `arg` itself when no slash is present. It returns a
> borrowed pointer rather than allocating a new name.

> [spec:ronin:def:samu.usage-fn]
> static void usage(void)

> [spec:ronin:sem:samu.usage-fn]
> Writes the supported command synopsis, using the global program name, to
> standard error and terminates the process with exit status 2.

> [spec:ronin:def:samu.warnflag-fn]
> static void warnflag(const char *flag)

> [spec:ronin:sem:samu.warnflag-fn]
> Recognizes only duplicate-build diagnostics: `dupbuild=err` disables the
> duplicate-build warning mode, while `dupbuild=warn` enables it. Any other
> warning switch is fatal.
