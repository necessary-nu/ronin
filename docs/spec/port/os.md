# os.h

> [spec:ronin:def:os.oschdir-fn]
> void oschdir(const char *)

> [spec:ronin:sem:os.oschdir-fn]
> Changes the process working directory to the supplied path; a failure is fatal
> and names the directory in its system-error diagnostic.

> [spec:ronin:def:os.osgetcwd-fn]
> void osgetcwd(char *, size_t)

> [spec:ronin:sem:os.osgetcwd-fn]
> Stores the current working directory in the caller-provided buffer of the
> supplied capacity; inability to obtain it is fatal.

> [spec:ronin:def:os.osmkdirs-fn]
> int osmkdirs(struct string *, _Bool)

> [spec:ronin:sem:os.osmkdirs-fn]
> Ensures all needed directory components of a mutable path exist. With
> `parent` true, leaves the final path component uncreated; otherwise also
> creates it. It returns zero on success and -1 after warning for a stat or
> mkdir failure other than an already-existing directory.

> [spec:ronin:def:os.osmtime-fn]
> int64_t osmtime(const char *)

> [spec:ronin:sem:os.osmtime-fn]
> Returns a file's modification time as nanoseconds since the Unix epoch. A
> missing file returns `MTIME_MISSING`; other stat failures are fatal.

> [spec:ronin:def:os.osnproc-fn]
> long osnproc(void)

> [spec:ronin:sem:os.osnproc-fn]
> Returns the operating system's count of online processors when available, or
> one on platforms without that query.

> [spec:ronin:def:os.osspawn-fn]
> pid_t osspawn(char *const argv[], int fd)

> [spec:ronin:sem:os.osspawn-fn]
> Starts `argv` in the inherited environment and returns its process ID. When
> `fd` is not -1, child standard input is `/dev/null` and standard output and
> error are redirected to that descriptor. Setup or spawning failure warns and
> returns -1.
