# parse.c, parse.h

> [spec:samurai:def:parse.checkversion-fn]
> static void checkversion(const char *ver)

> [spec:samurai:sem:parse.checkversion-fn]
> Parse `ver` with the `"%d.%d"` conversion, initializing the minor
> component to zero before parsing. At least the major conversion MUST
> succeed; otherwise report `invalid ninja_required_version` as a fatal
> error. The conversion is intentionally not required to consume all input:
> a major-only prefix is accepted with minor zero, and trailing text after a
> successful conversion is ignored.
>
> Compare the parsed pair lexicographically with the supported pair 1.9. If
> the major is greater than 1, or it is 1 and the minor is greater than 9,
> terminate with the fatal `ninja_required_version ... is newer than 1.9`
> diagnostic. All other parsed values, including negative values, return
> successfully and do not mutate parser state.

> [spec:samurai:def:parse.defaultnodes-fn]
> void defaultnodes(void fn(struct node *))

> [spec:samurai:sem:parse.defaultnodes-fn]
> If one or more explicit `default` declarations have been parsed, invoke
> `fn` once for each stored node, in declaration order. Do not deduplicate
> repeated declarations and do not alter the stored default-target list.
>
> If that list is empty, traverse the global `alledges` linked list in its
> existing order. For every edge, inspect its outputs in ascending array
> order and invoke `fn` for each output whose `nuse` count is zero. This is
> the implicit default set: generated nodes not consumed by any edge. The
> function neither filters phony edges nor records that a callback has run;
> the callback is responsible for any such policy.

> [spec:samurai:def:parse.parse-fn]
> void parse(const char *name, struct environment *env)

> [spec:samurai:sem:parse.parse-fn]
> Initialize a scanner over the file named by `name`, then repeatedly obtain
> the next top-level token. Dispatch `rule`, `build`, `include`, `subninja`,
> `default`, and `pool` tokens to `parserule`, `parseedge`, `parseinclude`
> with `newscope` false, `parseinclude` with `newscope` true,
> `parsedefault`, and `parsepool`, respectively.
>
> For a `VARIABLE` token, the scanner supplies an owned variable-name copy.
> Parse `=` and the rest of its line with `parselet`, immediately evaluate
> the resulting unevaluated string in `env`, and add the resulting owned
> string to `env` under that name. Before adding it, apply `checkversion` if
> and only if the name is `ninja_required_version`; the binding is still
> added after a successful check. Evaluation consumes the parsed
> `evalstring`, while the environment takes ownership of the name and value.
>
> On `EOF`, close the scanner and return. Scanner errors, undefined names,
> malformed directives, include failures, and fatal errors from any helper
> terminate parsing rather than being recovered locally. The function does
> not reset global parser state, so nested includes and successive calls
> share the caller-selected environment and accumulated graph/default state.

> [spec:samurai:def:parse.parsedefault-fn]
> static void parsedefault(struct scanner *s, struct environment *env)

> [spec:samurai:sem:parse.parsedefault-fn]
> Scan zero or more path expressions from the current position into the
> shared `paths` array, then grow the persistent default-node array by that
> many slots. For each path in scanner order, evaluate it in `env` (which
> consumes the `evalstring`), canonicalize the resulting string, and look up
> an already-known node by its bytes and length. A missing node is a fatal
> `unknown target` error; this directive never creates a node.
>
> Free each evaluated path string after its lookup and append the found node
> pointer to the persistent default list, preserving duplicates and order.
> Require and consume the terminating newline, then set the shared `npaths`
> count to zero. The backing `paths` allocation is retained for reuse.

> [spec:samurai:def:parse.parseedge-fn]
> static void parseedge(struct scanner *s, struct environment *env)

> [spec:samurai:sem:parse.parseedge-fn]
> Create an edge with a new child environment of `env`; edge creation also
> links it into the global edge list. Scan output paths. Record the count
> before an optional single `|` as `outimpidx`, scan paths after that `|` as
> implicit outputs, and set `nout` to the resulting total. Reject an empty
> output list. Require `:`, scan the rule name, resolve it through `env` and
> its parents, and fail fatally if absent; free the temporary rule-name copy
> after the lookup.
>
> Scan explicit input paths and set `inimpidx` to their count. Accept either
> `|` for implicit inputs, `||` for order-only inputs, or both in that order.
> After any implicit inputs, set `inorderidx` to the current input count;
> after optional order-only inputs, set `nin` to the complete input count.
> Require the line ending. For each indented binding line, scan its name and
> value, evaluate that value in the parent `env` (not in the edge child
> environment), and transfer the name and evaluated value into the edge
> environment. Thus these bindings are fixed while parsing and cannot see
> earlier bindings on the same edge through this evaluation step.
>
> Allocate the output-node array. In path order, evaluate every output in
> the edge environment, canonicalize it, and intern it as a graph node. If a
> node already has a generating edge, fail with `multiple rules generate`
> unless `parseopts.dupbuildwarn` is set. With that option set, warn instead,
> discard that output from this edge by decrementing `nout`, decrement
> `outimpidx` when the rejected output was still explicit, and continue
> without advancing the destination-output index. Otherwise set the node's
> generator to this edge and store it in the next output slot. Interning takes
> ownership of each canonical path string (or frees it when it found an
> existing node).
>
> Allocate the input-node array, then evaluate, canonicalize, and intern all
> inputs in their scanned order. Store each node and record the edge as a use
> of that node. Finally reset shared `npaths` to zero. Evaluate the edge's
> `pool` variable with escaping enabled; if it is present, resolve and assign
> the named pool, with an unknown pool being fatal. If absent, retain the
> null pool assigned at edge creation. Parsed path expressions are consumed
> by evaluation; the reusable `paths` backing array itself remains allocated.

> [spec:samurai:def:parse.parseinclude-fn]
> static void parseinclude(struct scanner *s, struct environment *env, bool newscope)

> [spec:samurai:sem:parse.parseinclude-fn]
> Scan exactly one path-mode string. An empty result is a scanner error
> `expected include path`. Require its terminating newline, then evaluate
> the path in the supplied environment; evaluation consumes the parsed
> `evalstring` and yields an owned path string.
>
> For `newscope == false`, recursively parse that file in the same
> environment, so its variables and rules modify the caller's scope. For
> `newscope == true`, first create a child environment and recursively parse
> in that child, isolating bindings and rules added by the included file from
> the parent while retaining parent lookup. Keep the evaluated path alive for
> the recursive call because the scanner borrows its pathname, then free it
> immediately after that call returns. Any recursive parse error terminates
> the whole operation.

> [spec:samurai:def:parse.parseinit-fn]
> void parseinit(void)

> [spec:samurai:sem:parse.parseinit-fn]
> Release only the dynamically allocated array that stores explicit default
> node pointers, then set its pointer to null and its count to zero. The
> nodes themselves are borrowed graph objects and are not freed here. This
> function does not reset parse options, environments, graph state, or the
> scanner's shared path storage.

> [spec:samurai:def:parse.parselet-fn]
> static void parselet(struct scanner *s, struct evalstring **val)

> [spec:samurai:sem:parse.parselet-fn]
> Require and consume `=` (including any following scanner whitespace), scan
> a non-path string from the remainder of the logical line, store its
> possibly-null `evalstring` head through `val`, and require/consume the
> line ending. A blank right-hand side therefore stores null; otherwise the
> returned linked expression retains literal and variable parts for the
> caller to evaluate or retain. This helper does not evaluate or free the
> expression it stores.

> [spec:samurai:def:parse.parseoptions]
> struct parseoptions {
>   _Bool dupbuildwarn;
> }

> [spec:samurai:def:parse.parsepool-fn]
> static void parsepool(struct scanner *s, struct environment *env)

> [spec:samurai:sem:parse.parsepool-fn]
> Create and register a pool using the scanned name; pool creation takes the
> name and rejects a duplicate pool. Require the declaration newline, then
> process each indented binding. Only a binding named `depth` is accepted.
> For it, parse the right-hand side, evaluate it in `env`, and convert the
> result with base-10 `strtol`. Reject the value only when unconsumed text
> remains; no range, sign, or `errno` validation is performed. Assign the
> converted value to `maxjobs`, so later `depth` bindings overwrite earlier
> ones, then free the evaluated string.
>
> Any other binding name is a fatal `unexpected pool variable` error. After
> the indented block, require `maxjobs` to be nonzero; zero (including an
> empty value parsed as zero) is a fatal `pool ... has no depth` error,
> whereas negative nonzero values pass this check. The per-line variable-name
> copy is used only for the comparison and is not retained by the pool.

> [spec:samurai:def:parse.parserule-fn]
> static void parserule(struct scanner *s, struct environment *env)

> [spec:samurai:sem:parse.parserule-fn]
> Create a rule from the scanned name (transferring ownership of that name to
> the rule), require the declaration newline, then process every indented
> binding line. For each line, scan its name and unevaluated right-hand-side
> expression with `parselet`, and add both to the rule; values are deliberately
> retained unevaluated for later edge-variable expansion.
>
> While processing only non-null values, remember whether the exact names
> `command`, `rspfile`, and `rspfile_content` occurred. After the block,
> reject a rule without a non-null `command`, and reject a rule with exactly
> one of `rspfile` and `rspfile_content`, using fatal diagnostics. Empty
> special-variable assignments are stored but do not satisfy these checks.
> Other names are accepted without restriction. Finally add the rule to
> `env`; adding a duplicate rule is fatal and otherwise transfers the rule's
> ownership to the environment.
