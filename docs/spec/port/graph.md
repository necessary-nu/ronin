# graph.c, graph.h

> [spec:samurai:def:graph.delnode-fn]
> static void delnode(void *p)

> [spec:samurai:sem:graph.delnode-fn]
> Treat `p` as an owned node.  If `shellpath` is a separately allocated
> string (`shellpath != path`), release it; if it aliases `path`, do not
> release it twice.  Then release the consumer-edge array, the owned canonical
> path string, and the node.  The generating edge is not owned here.

> [spec:samurai:def:graph.edge]
> struct edge {
>   struct rule *rule;
>   struct pool *pool;
>   struct environment *env;
>   struct node **out, **in;
>   size_t nout, nin;
>   size_t outimpidx;
>   size_t inimpidx, inorderidx;
>   uint64_t hash;
>   size_t nblock;
>   size_t nprune;
>   enum { FLAG_WORK = 1 << 0, /* scheduled for build */ FLAG_HASH = 1 << 1, /* calculated the command hash */ FLAG_DIRTY_IN = 1 << 3, /* dirty input */ FLAG_DIR...;
>   struct edge *worknext;
>   struct edge *allnext;
> }

> [spec:samurai:def:graph.edgeadddeps-fn]
> void edgeadddeps(struct edge *e, struct node **deps, size_t ndeps)

> [spec:samurai:sem:graph.edgeadddeps-fn]
> For every supplied dependency, ensure that it has a generating edge: when
> `gen` is null, create a synthetic phony edge for that node and assign it to
> `gen`.  Append `e` to the node's use list (without deduplicating repeated
> dependencies).  Grow `e->in`, insert copies of the `deps` pointers at
> `inorderidx`, and shift the old order-only suffix right by `ndeps`.  Advance
> `inorderidx` and `nin` by `ndeps`, while leaving `inimpidx` unchanged.  The
> caller retains the `deps` array; the edge owns only its enlarged input array.

> [spec:samurai:def:graph.edgehash-fn]
> void edgehash(struct edge *e)

> [spec:samurai:sem:graph.edgehash-fn]
> If `FLAG_HASH` is already set, leave `hash` unchanged.  Otherwise set that
> flag before evaluating anything, obtain the shell-escaped `command`, and
> terminate fatally if it is absent.  Obtain shell-escaped
> `rspfile_content`; if it is nonempty, hash the byte sequence
> `command + ";rspfile=" + rspfile_content`, otherwise hash the command bytes
> alone.  Store the RapidHash v1 result in `e->hash`; the temporary joined
> string, when used, is released after hashing.

> [spec:samurai:def:graph.graphinit-fn]
> void graphinit(void)

> [spec:samurai:sem:graph.graphinit-fn]
> Discard the previous node table, invoking `delnode` for every stored node.
> Then repeatedly remove the head of `alledges` and release its output array,
> input array, and edge object; environments, rules, pools, and nodes are
> owned elsewhere and are not released through an edge.  Finally replace the
> node table with a new hash table of initial capacity 1024.  On return the
> global edge list is empty and the new table is ready for node interning.

> [spec:samurai:def:graph.mkedge-fn]
> struct edge * mkedge(struct environment *parent)

> [spec:samurai:sem:graph.mkedge-fn]
> Allocate an edge and create a fresh child environment whose parent is
> `parent`.  Initialize `pool` to null, both node arrays to null, both counts
> to zero, and `flags` to zero.  Prepend the edge to the global `alledges`
> list and return it.  Rule selection, input/output boundary indices, and the
> remaining scheduling/hash fields are established later by the parser or
> builder; this constructor does not give them semantic defaults.

> [spec:samurai:def:graph.mknode-fn]
> struct node * mknode(struct string *path)

> [spec:samurai:sem:graph.mknode-fn]
> Intern `path` by its exact byte length and contents.  If an equal path is
> already present, release the supplied string and return the existing node.
> Otherwise the new node takes ownership of `path`, starts with no shell-path
> cache, producer, or consumers, has `mtime = MTIME_UNKNOWN`,
> `logmtime = MTIME_MISSING`, `hash = 0`, and dependency-log `id = -1`, and is
> stored in the node table before being returned.  Scheduling sets `dirty`
> when it first evaluates the node; construction does not derive it.

> [spec:samurai:def:graph.mkphony-fn]
> static struct edge * mkphony(struct node *n)

> [spec:samurai:sem:graph.mkphony-fn]
> Create a normal edge rooted in `rootenv`, designate the global `phonyrule`,
> and make `n` its sole output.  Set both input boundaries to zero and set
> `outimpidx` and `nout` to one, so the sole output is non-implicit.  The
> function returns the new edge (already linked into `alledges`) but does not
> itself assign it to `n->gen`; its caller does so.

> [spec:samurai:def:graph.node]
> struct node {
>   struct string *path, *shellpath;
>   int64_t mtime, logmtime;
>   struct edge *gen, **use;
>   size_t nuse;
>   uint64_t hash;
>   int32_t id;
>   _Bool dirty;
> }

> [spec:samurai:def:graph.nodeget-fn]
> struct node * nodeget(const char *path, size_t len)

> [spec:samurai:sem:graph.nodeget-fn]
> Look up a node by an exact path byte sequence in the global intern table.
> A zero `len` means use the C-string length of `path`; a nonzero length is
> used verbatim.  Return the interned node pointer when present, or null when
> no equal key has been inserted.  The lookup neither allocates nor changes
> ownership.

> [spec:samurai:def:graph.nodestat-fn]
> void nodestat(struct node *n)

> [spec:samurai:sem:graph.nodestat-fn]
> Query the filesystem modification time of the node's canonical path and
> overwrite `n->mtime` with that result.  Missing files become
> `MTIME_MISSING`; filesystem-stat failures other than a missing file follow
> the platform stat helper's fatal-error behavior.  No other node field is
> changed.

> [spec:samurai:def:graph.nodeuse-fn]
> void nodeuse(struct node *n, struct edge *e)

> [spec:samurai:sem:graph.nodeuse-fn]
> Append `e` to `n`'s consumer list and increment `nuse`; duplicate consumer
> entries are meaningful and are retained.  Grow the backing array just before
> appending at sizes 0, 1, 2, 4, and so on, giving capacities 1, 2, 4, 8, ... .
> The node owns the resized array, and existing entries keep their order.
