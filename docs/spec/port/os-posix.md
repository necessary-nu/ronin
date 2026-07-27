# os-posix.c

> [spec:samurai:def:os-posix.oschdir-fn]
> void oschdir(const char *dir)

> [spec:samurai:sem:os-posix.oschdir-fn]
> Calls `chdir` for `dir`; any failure is reported with the path and terminates
> the process.

> [spec:samurai:def:os-posix.osgetcwd-fn]
> void osgetcwd(char *buf, size_t len)

> [spec:samurai:sem:os-posix.osgetcwd-fn]
> Calls `getcwd` into `buf` with `len` bytes and terminates with a system-error
> diagnostic on failure.

> [spec:samurai:def:os-posix.osmkdirs-fn]
> int osmkdirs(struct string *path, bool parent)

> [spec:samurai:sem:os-posix.osmkdirs-fn]
> Walks backward through the mutable path until it finds an existing prefix or a
> stat error, then walks forward creating the missing slash-delimited
> components with mode 0777. The final component is excluded when `parent` is
> true. Existing directories are accepted; non-ENOENT stat errors and failed
> mkdir calls warn and cause a final -1 while later components are still
> restored in the path. Otherwise returns zero.

> [spec:samurai:def:os-posix.osmtime-fn]
> int64_t osmtime(const char *name)

> [spec:samurai:sem:os-posix.osmtime-fn]
> Stats `name` and returns its platform-specific seconds-plus-nanoseconds mtime
> normalized to nanoseconds. `ENOENT` yields `MTIME_MISSING`; any other stat
> error is fatal.

> [spec:samurai:def:os-posix.osnproc-fn]
> long osnproc(void)

> [spec:samurai:sem:os-posix.osnproc-fn]
> Returns `sysconf(_SC_NPROCESSORS_ONLN)` when that capability exists, otherwise
> returns one; it forwards a negative `sysconf` result unchanged.

> [spec:samurai:def:os-posix.osspawn-fn]
> pid_t osspawn(char *const argv[], int outfd)

> [spec:samurai:sem:os-posix.osspawn-fn]
> Uses `posix_spawn` where available, otherwise fork/execs. With an output
> descriptor it arranges `/dev/null` as stdin and duplicates that descriptor to
> stdout and stderr; without one it inherits standard streams. It uses the
> current environment and returns the child PID on success. Any parent-side
> setup, fork, or spawn error warns and returns -1; a child-side setup or exec
> error exits with status 1.
