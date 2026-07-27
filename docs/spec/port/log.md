# log.c

> [spec:samurai:def:log.logclose-fn]
> void logclose(void)

> [spec:samurai:sem:log.logclose-fn]
> Flushes the build log, treats a stream error as fatal, and closes the file.

> [spec:samurai:def:log.loginit-fn]
> void loginit(const char *builddir)

> [spec:samurai:sem:log.loginit-fn]
> Opens `<builddir>/.ninja_log` (or the root name) read/write, checks its
> versioned header, then parses tab-separated records. For each current
> generated output it retains the recorded restat mtime and hexadecimal command
> hash; malformed fields warn and are skipped. A missing, incompatible, unreadable,
> or excessively redundant log is rebuilt atomically via `.ninja_log.tmp` with
> the current header and every hashed output. I/O, flush, or rename failures in
> the rebuild are fatal.

> [spec:samurai:def:log.logrecord-fn]
> void logrecord(struct node *n)

> [spec:samurai:sem:log.logrecord-fn]
> Appends one version-7 Ninja log record for the node, using zero start/end
> times and tab-separated log mtime, canonical path, and hexadecimal command
> hash.

> [spec:samurai:def:log.nextfield-fn]
> static char * nextfield(char **end)

> [spec:samurai:sem:log.nextfield-fn]
> Returns the field beginning at `*end`, splitting it at the next tab or newline
> by replacing that delimiter with NUL and advancing `*end`. An empty starting
> field warns about log corruption and returns null.
