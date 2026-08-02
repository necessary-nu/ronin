# util.c, util.h

> [spec:ronin:def:util.bufadd-fn]
> void bufadd(struct buffer *buf, char c)

> [spec:ronin:sem:util.bufadd-fn]
> Ensures space for one byte, growing capacity from zero to 256 and thereafter
> doubling it. Allocation failure is fatal. It then writes `c` at the current
> length and increments the length; it does not append a terminator.

> [spec:ronin:def:util.buffer]
> struct buffer {
>   char *data;
>   size_t len, cap;
> }

> [spec:ronin:def:util.canonpath-fn]
> void canonpath(struct string *path)

> [spec:ronin:sem:util.canonpath-fn]
> Canonicalizes the mutable, NUL-terminated path in place without consulting
> the filesystem. An empty path is fatal. It preserves an initial slash,
> removes repeated slashes and `.` components, and removes a preceding retained
> component for each `..`; a leading or otherwise unmatched `..` is retained.
> It records at most 60 component starts and fails if that limit is exceeded,
> removes any final separator, writes a NUL terminator, and updates `path->n`.
> A result with no components becomes `.`.

> [spec:ronin:def:util.delevalstr-fn]
> void delevalstr(void *ptr)

> [spec:ronin:sem:util.delevalstr-fn]
> Walks the linked list beginning at `ptr`. For every entry it saves `next`,
> frees `var` when that field is present or otherwise frees `str`, then frees
> the entry itself before continuing. Thus each entry owns exactly one of its
> variable-name string and literal-string object.

> [spec:ronin:def:util.evalstring]
> struct evalstring {
>   char *var;
>   struct string *str;
>   struct evalstring *next;
> }

> [spec:ronin:def:util.fatal-fn]
> void fatal(const char *fmt, ...)

> [spec:ronin:sem:util.fatal-fn]
> Formats the variadic diagnostic through the shared warning formatter, then
> terminates the process with exit status 1.

> [spec:ronin:def:util.reallocarray-fn]
> static void * reallocarray_(void *p, size_t n, size_t m)

> [spec:ronin:sem:util.reallocarray-fn]
> Checks whether multiplying element count `n` by element size `m` overflows.
> On overflow it sets `errno` to out-of-memory and returns null; otherwise it
> delegates to reallocation of `n * m` bytes and returns that result.

> [spec:ronin:def:util.string]
> struct string {
>   size_t n;
>   char s[];
> }

> [spec:ronin:def:util.vwarn-fn]
> static void vwarn(const char *fmt, va_list ap)

> [spec:ronin:sem:util.vwarn-fn]
> Writes the global program name and `: ` to standard error, then renders the
> supplied variadic format. If the format ends in `:`, appends a space and the
> current system-error text; otherwise appends a newline.

> [spec:ronin:def:util.warn-fn]
> void warn(const char *fmt, ...)

> [spec:ronin:sem:util.warn-fn]
> Forwards its variadic arguments to the shared warning formatter and then
> completes normally.

> [spec:ronin:def:util.writefile-fn]
> int writefile(const char *name, struct string *s)

> [spec:ronin:sem:util.writefile-fn]
> Opens `name` for writing, creating or truncating it. If opening fails, emits
> a system-error warning and returns -1. When a string is supplied, writes
> exactly its recorded byte length and flushes the stream; a short write or
> flush failure emits a warning and changes the result to -1. It closes the
> stream in all successful-open cases and otherwise returns 0.

> [spec:ronin:def:util.xasprintf-fn]
> int xasprintf(char **s, const char *fmt, ...)

> [spec:ronin:sem:util.xasprintf-fn]
> Formats the variadic arguments once to determine the required character
> count, allocates that count plus a terminator through the fatal allocator,
> then formats again into the allocated buffer and stores it through `s`.
> Either formatting failure, or an unexpected second-pass length, is fatal; on
> success it returns the character count excluding the terminator.

> [spec:ronin:def:util.xmalloc-fn]
> void * xmalloc(size_t n)

> [spec:ronin:sem:util.xmalloc-fn]
> Allocates `n` bytes. A null allocation result is reported as a system-memory
> error and terminates the process; otherwise the allocated pointer is returned.

> [spec:ronin:def:util.xmemdup-fn]
> char * xmemdup(const char *s, size_t n)

> [spec:ronin:sem:util.xmemdup-fn]
> Allocates exactly `n` bytes through the fatal allocator, copies exactly `n`
> bytes from `s` into that storage, and returns the newly owned buffer.

> [spec:ronin:def:util.xreallocarray-fn]
> void * xreallocarray(void *p, size_t n, size_t m)

> [spec:ronin:sem:util.xreallocarray-fn]
> Reallocates `p` for `n` elements of size `m` using the checked array
> allocator. A null result, whether from overflow or allocation failure, is
> reported as a system-memory error and terminates the process; otherwise the
> resulting pointer is returned.
