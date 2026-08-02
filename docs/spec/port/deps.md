# deps.c

> [spec:ronin:def:deps.depsclose-fn]
> void depsclose(void)

> [spec:ronin:sem:deps.depsclose-fn]
> Flushes the open dependency log, treating a stream error as fatal, then closes
> the stream.

> [spec:ronin:def:deps.depsinit-fn]
> void depsinit(const char *builddir)

> [spec:ronin:sem:deps.depsinit-fn]
> Opens `<builddir>/.ninja_deps` (or the root name), discards the previous
> stream state, and reads its header, version, and binary node/dependency
> records. It validates sizes, checksums, IDs, ordering, and referenced nodes;
> valid node records are interned and valid dependency records are retained only
> for edges configured with `deps`. A missing, invalid, truncated, obsolete, or
> excessively redundant log is rewritten atomically through `.ninja_deps.tmp`:
> it emits the header/version and compacts only entries that still have
> dependencies, assigning fresh IDs to outputs and their dependencies. I/O or
> rename failures during this recovery are fatal.

> [spec:ronin:def:deps.depsload-fn]
> void depsload(struct edge *e)

> [spec:ronin:sem:deps.depsload-fn]
> Runs at most once per edge and marks that fact. For an edge with `deps`, it
> uses the first output's recorded dependency list only when its stored mtime is
> at least the output mtime; otherwise it optionally explains the stale record.
> Without `deps`, it reads the expanded `depfile` when present. A found list is
> inserted as implicit inputs; an absent or invalid required list marks the
> first output and edge's outputs dirty.

> [spec:ronin:def:deps.depsparse-fn]
> static struct nodearray * depsparse(const char *name, bool allowmissing)

> [spec:ronin:sem:deps.depsparse-fn]
> Parses a make-style dependency file into a reusable node array. A missing file
> returns an empty list only when `allowmissing` is true; other open failures
> return null. It accepts one logical output followed by colon-separated input
> paths, continued lines, doubled dollars, and compiler-style backslash escapes;
> it rejects variable references, malformed escapes, invalid target characters,
> differing multiple outputs, and read errors with a warning. Each parsed input
> path is interned as a node. Success returns the static array; failure returns
> null after closing the file.

> [spec:ronin:def:deps.depsrecord-fn]
> void depsrecord(struct edge *e)

> [spec:ronin:sem:deps.depsrecord-fn]
> Does nothing unless the edge requests a nonempty `gcc` dependency mode and a
> nonempty depfile; unsupported modes or a missing depfile warn and return. It
> parses the depfile allowing absence, removes it unless preservation is set,
> and compares the first output's mtime and dependency identity/order with any
> existing record. It assigns IDs to the output and all parsed dependencies;
> when anything changed, appends a binary dependency record and flushes it,
> treating flush failure as fatal.

> [spec:ronin:def:deps.depswrite-fn]
> static void depswrite(const void *p, size_t n, size_t m)

> [spec:ronin:sem:deps.depswrite-fn]
> Writes exactly `m` elements of `n` bytes to the dependency-log stream and
> terminates fatally if the complete write does not occur.

> [spec:ronin:def:deps.entry]
> struct entry {
>   struct node *node;
>   struct nodearray deps;
>   int64_t mtime;
> }

> [spec:ronin:def:deps.nodearray]
> struct nodearray {
>   struct node **node;
>   size_t len;
> }

> [spec:ronin:def:deps.recorddeps-fn]
> static void recorddeps(struct node *out, struct nodearray *deps, int64_t mtime)

> [spec:ronin:sem:deps.recorddeps-fn]
> Appends one high-bit-marked binary dependency record for `out`: the record
> contains its byte size, output ID, low and high 32-bit halves of `mtime`, and
> each dependency ID in order. A record reaching the configured maximum size is
> fatal.

> [spec:ronin:def:deps.recordid-fn]
> static bool recordid(struct node *n)

> [spec:ronin:sem:deps.recordid-fn]
> Returns false when the node already has an ID. Otherwise assigns the next
> sequential ID (failing at the 32-bit limit), writes a padded node record whose
> payload is the path and whose final word is the bitwise-complement checksum of
> that ID, then returns true. Paths that cannot fit in a record are fatal.
