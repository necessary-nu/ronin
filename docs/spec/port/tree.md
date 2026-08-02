# tree.c, tree.h

> [spec:ronin:def:tree.balance-fn]
> static int balance(struct treenode **p)

> [spec:ronin:sem:tree.balance-fn]
> Computes the heights of both children of the referenced node. When their
> difference is at most one, replaces the node's stored height with one plus
> the greater child height and returns that height's change. When the tree is
> unbalanced, rotates toward the deeper child and returns the rotation's height
> change; the referenced root may therefore change.

> [spec:ronin:def:tree.deltree-fn]
> void deltree(struct treenode *n, void delkey(void *), void delval(void *))

> [spec:ronin:sem:tree.deltree-fn]
> Treats a null node as an empty tree. For each non-null node it first invokes
> the optional key and value destructors on that node's payloads, then
> recursively destroys child 0 and child 1, and finally frees the node itself.
> It does not otherwise own or inspect the payloads.

> [spec:ronin:def:tree.height-fn]
> static inline int height(struct treenode *n)

> [spec:ronin:sem:tree.height-fn]
> Returns zero for a null node and otherwise returns the node's stored AVL
> height.

> [spec:ronin:def:tree.rot-fn]
> static int rot(struct treenode **p, struct treenode *x, int dir /* deeper side */)

> [spec:ronin:sem:tree.rot-fn]
> Rebalances an over-deep child in `dir` and replaces `*p` with the new subtree
> root. It uses a double rotation when the inner grandchild is taller than the
> outer grandchild, otherwise a single rotation. In both cases it reconnects
> all affected subtrees, recalculates the participating heights, and returns
> the new root height minus the former root height.

> [spec:ronin:def:tree.treefind-fn]
> struct treenode * treefind(struct treenode *n, const char *key)

> [spec:ronin:sem:tree.treefind-fn]
> Walks the binary-search tree by lexicographically comparing `key` with each
> node key: equal returns that node, a greater key follows child 1, and a
> lesser key follows child 0. It returns null after reaching an absent child.

> [spec:ronin:def:tree.treeinsert-fn]
> void * treeinsert(struct treenode **rootp, char *key, void *value)

> [spec:ronin:sem:tree.treeinsert-fn]
> Searches for `key` using lexicographic order while recording the links on the
> search path. If an equal key exists, replaces its value and returns the old
> value without taking ownership of the replacement. Otherwise allocates a
> leaf holding the supplied key and value, initializes both children to null and
> its height to one, attaches it at the missing link, then walks the recorded
> ancestors upward until their height no longer changes, rebalancing as needed.
> It returns null for a new insertion.

> [spec:ronin:def:tree.treenode]
> struct treenode {
>   char *key;
>   void *value;
>   struct treenode *child[2];
>   int height;
> }
