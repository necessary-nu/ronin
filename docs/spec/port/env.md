# env.c, env.h

> [spec:samurai:def:env.addpool-fn]
> static void addpool(struct pool *p)

> [spec:samurai:sem:env.addpool-fn]
> Inserts the pool into the global name-indexed pool tree. A name already in
> that tree is a fatal redefinition error.

> [spec:samurai:def:env.addvar-fn]
> static void addvar(struct treenode **tree, char *var, void *val)

> [spec:samurai:sem:env.addvar-fn]
> Inserts `var` and `val` into the supplied binding tree. If the key already
> exists, retains the new value, frees the replaced value, and leaves key
> ownership with the tree.

> [spec:samurai:def:env.delpool-fn]
> static void delpool(void *ptr)

> [spec:samurai:sem:env.delpool-fn]
> Releases an owned pool's name and allocation. The global console pool is
> static and is deliberately left untouched.

> [spec:samurai:def:env.delrule-fn]
> static void delrule(void *ptr)

> [spec:samurai:sem:env.delrule-fn]
> Leaves the static phony rule intact. For every other rule, destroys its
> binding tree (freeing variable names and evaluated-string lists), then frees
> its owned name and the rule.

> [spec:samurai:def:env.edgevar-fn]
> struct string * edgevar(struct edge *e, char *var, bool escape)

> [spec:samurai:sem:env.edgevar-fn]
> Resolves edge variables with special computed values: `in`, `in_newline`, and
> `out` join explicit inputs or outputs using the requested separator and
> optional shell escaping. Other names prefer edge-local bindings, then rule
> bindings, then the parent environment chain. Rule bindings are evaluated by
> recursively resolving their fragments on this edge; a temporary sentinel
> detects recursive rule-variable references and makes them fatal. The result
> is a newly merged string unless it came directly from a local/environment
> binding.

> [spec:samurai:def:env.envaddrule-fn]
> void envaddrule(struct environment *env, struct rule *r)

> [spec:samurai:sem:env.envaddrule-fn]
> Inserts a rule into the environment's local rule tree and terminates fatally
> if that name was already defined in the same environment.

> [spec:samurai:def:env.envaddvar-fn]
> void envaddvar(struct environment *env, char *var, struct string *val)

> [spec:samurai:sem:env.envaddvar-fn]
> Adds or replaces a local variable binding in `env`; replacing a binding frees
> the old string value.

> [spec:samurai:def:env.enveval-fn]
> struct string * enveval(struct environment *env, struct evalstring *str)

> [spec:samurai:sem:env.enveval-fn]
> Resolves every variable fragment in an unevaluated-string list through `env`,
> totals the lengths of present fragments, concatenates them into one allocated,
> NUL-terminated string, destroys the input fragment list, and returns the
> result. Missing variables contribute no bytes.

> [spec:samurai:def:env.envinit-fn]
> void envinit(void)

> [spec:samurai:sem:env.envinit-fn]
> Destroys all previously allocated environments, their variable and rule
> trees, and the pool tree (while preserving static phony/console objects).
> It then creates a fresh root environment, installs the static `phony` rule,
> clears the pool root, and registers the one-slot `console` pool.

> [spec:samurai:def:env.environment]
> struct environment {
>   struct environment *parent;
>   struct treenode *bindings;
>   struct treenode *rules;
>   struct environment *allnext;
> }

> [spec:samurai:def:env.envrule-fn]
> struct rule * envrule(struct environment *env, char *name)

> [spec:samurai:sem:env.envrule-fn]
> Searches the environment's local rule tree and then successive parents,
> returning the nearest matching rule or null when none exists.

> [spec:samurai:def:env.envvar-fn]
> struct string * envvar(struct environment *env, char *var)

> [spec:samurai:sem:env.envvar-fn]
> Searches local variable bindings and then successive parent environments,
> returning the nearest matching string or null when the name is unbound.

> [spec:samurai:def:env.mkenv-fn]
> struct environment * mkenv(struct environment *parent)

> [spec:samurai:sem:env.mkenv-fn]
> Allocates an environment with the specified parent and empty local binding and
> rule trees, prepends it to the global ownership list, and returns it.

> [spec:samurai:def:env.mkpool-fn]
> struct pool * mkpool(char *name)

> [spec:samurai:sem:env.mkpool-fn]
> Allocates a named pool with zero active jobs, an unlimited/unspecified maximum
> represented by zero, and an empty waiting queue; registers it globally and
> returns it.

> [spec:samurai:def:env.mkrule-fn]
> struct rule * mkrule(char *name)

> [spec:samurai:sem:env.mkrule-fn]
> Allocates a rule that owns `name` and starts with an empty binding tree.

> [spec:samurai:def:env.pool]
> struct pool {
>   char *name;
>   int numjobs, maxjobs;
>   struct edge *work;
> }

> [spec:samurai:def:env.poolget-fn]
> struct pool * poolget(char *name)

> [spec:samurai:sem:env.poolget-fn]
> Looks up a pool by name in the global pool tree and returns it; an unknown
> name is fatal.

> [spec:samurai:def:env.rule]
> struct rule {
>   char *name;
>   struct treenode *bindings;
> }

> [spec:samurai:def:env.ruleaddvar-fn]
> void ruleaddvar(struct rule *r, char *var, struct evalstring *val)

> [spec:samurai:sem:env.ruleaddvar-fn]
> Adds or replaces an unevaluated-string binding in the rule; replacement
> destroys the old evaluated-string list.
