# tool.c, tool.h

> [spec:samurai:def:tool.clean-fn]
> static int clean(int argc, char *argv[])

> [spec:samurai:sem:tool.clean-fn]
> Implements `-t clean`. It accepts `-g` to include generator edges and `-r` to
> select rules; invalid options print usage and return 2. Rule mode requires
> names and removes every output, response file, and depfile of matching edges,
> warning and returning 1 for unknown rules. Target mode recursively removes
> generated target trees. With neither, it cleans every non-phony edge except
> generator edges unless `-g` is set. Filesystem removal failures make the
> result 1.

> [spec:samurai:def:tool.cleanedge-fn]
> static int cleanedge(struct edge *e)

> [spec:samurai:sem:tool.cleanedge-fn]
> Attempts to remove every edge output plus its expanded response-file and
> depfile paths, returning -1 if any individual removal fails and zero otherwise.

> [spec:samurai:def:tool.cleanpath-fn]
> static int cleanpath(struct string *path)

> [spec:samurai:sem:tool.cleanpath-fn]
> Treats a null path as success. For a present path, removes it and prints the
> removal on success; a missing file is also success, while any other error
> warns and returns -1.

> [spec:samurai:def:tool.cleantarget-fn]
> static int cleantarget(struct node *n)

> [spec:samurai:sem:tool.cleantarget-fn]
> Does nothing for source nodes and phony-generated nodes. For another generated
> node, removes its path and recursively applies the same operation to every
> input of its generating edge, returning -1 if any removal in the traversal
> fails.

> [spec:samurai:def:tool.commands-fn]
> static int commands(int argc, char *argv[])

> [spec:samurai:sem:tool.commands-fn]
> Prints build commands in dependency-first depth-first order for the requested
> targets (arguments after the tool name), or for default targets when none are
> supplied. Unknown targets are fatal. It flushes standard output and treats a
> write error as fatal before returning zero.

> [spec:samurai:def:tool.compdb-fn]
> static int compdb(int argc, char *argv[])

> [spec:samurai:sem:tool.compdb-fn]
> Implements `-t compdb [-x] rules...` by emitting a JSON array for edges with
> inputs whose rule name is selected. Each object contains the current directory,
> expanded command, first input path, and first output path with JSON quoting.
> With `-x`, an `@rspfile` occurrence in a command is replaced by the expanded
> response-file content, joining embedded newlines as spaces. Invalid options
> print usage and return 2; output write errors are fatal.

> [spec:samurai:def:tool.graph-fn]
> static int graph(int argc, char *argv[])

> [spec:samurai:sem:tool.graph-fn]
> Emits a Graphviz directed graph with fixed Ninja layout defaults, then renders
> requested targets (after the tool name) or defaults via depth-first graph
> traversal. Unknown targets and output write errors are fatal; success returns
> zero.

> [spec:samurai:def:tool.graphnode-fn]
> static void graphnode(struct node *n)

> [spec:samurai:sem:tool.graphnode-fn]
> Emits a quoted Graphviz node for `n`. It stops at source nodes or an already
> visited generating edge; otherwise marks that edge visited, recursively emits
> inputs, and represents one-input/one-output edges as a labeled direct arc.
> Other edges become ellipse action nodes connected to outputs and from inputs,
> with order-only inputs rendered dotted.

> [spec:samurai:def:tool.printquoted-fn]
> static void printquoted(const char *s, size_t n, bool join)

> [spec:samurai:sem:tool.printquoted-fn]
> Writes at most `n` bytes, stopping early at NUL. It prefixes quotes and
> backslashes with a backslash, writes other bytes unchanged, and either drops
> newlines or converts them to spaces when `join` is true.

> [spec:samurai:def:tool.query-fn]
> static int query(int argc, char *argv[])

> [spec:samurai:sem:tool.query-fn]
> Requires at least one target after the tool name, otherwise prints usage and
> exits 2. For each known node it prints the node name, its generating rule and
> all generator inputs when present, followed by every output of every consuming
> edge. Unknown targets are fatal; success returns zero.

> [spec:samurai:def:tool.targetcommands-fn]
> static void targetcommands(struct node *n)

> [spec:samurai:sem:tool.targetcommands-fn]
> For a generated node not already visited, marks its edge visited, recursively
> visits all inputs, then prints the edge's nonempty expanded command. Source
> nodes and repeated edges produce no output.

> [spec:samurai:def:tool.targets-fn]
> static int targets(int argc, char *argv[])

> [spec:samurai:sem:tool.targets-fn]
> Implements `-t targets` modes. Default or `depth [maxdepth]` prints every
> leaf consumer target with a recursively indented producer/input tree; the
> optional depth must be numeric. `rule [name]` prints source inputs of every
> edge when no name is supplied, or outputs of edges with that rule when one is.
> `all` prints every output with its rule. Invalid arity or mode prints usage
> and exits 2; write errors are fatal.

> [spec:samurai:def:tool.targetsdepth-fn]
> static void targetsdepth(struct node *n, size_t depth, size_t indent)

> [spec:samurai:sem:tool.targetsdepth-fn]
> Prints `indent` two-space levels, then prints a generated node with its rule
> or a source node alone. For a generated node and a depth other than one, it
> recursively prints every input one indentation deeper with depth decremented.

> [spec:samurai:def:tool.targetsusage-fn]
> static void targetsusage(void)

> [spec:samurai:sem:tool.targetsusage-fn]
> Writes all supported `targets` invocation forms to standard error and exits
> with status 2.

> [spec:samurai:def:tool.tool]
> struct tool {
>   const char *name;
> }

> [spec:samurai:def:tool.tool.run-fn]
> int (*run)(int, char *[])

> [spec:samurai:sem:tool.tool.run-fn]
> Invokes the selected tool implementation with its argument count and vector,
> returning that implementation's status.

> [spec:samurai:def:tool.toolget-fn]
> const struct tool * toolget(const char *name)

> [spec:samurai:sem:tool.toolget-fn]
> Linearly searches the static tool table by exact name and returns the matching
> descriptor. An unknown name is fatal.
