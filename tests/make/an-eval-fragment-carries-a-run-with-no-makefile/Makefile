# Never read. The run happens in `empty/`, which `setup` creates and leaves
# with nothing in it, so the invocation has no makefile at all.
#
# No makefile is not itself a refusal. GNU Make reads whatever there is —
# `--eval` text included — and gives up only at the end of it, when there is no
# goal to aim at (`if (!goals)`, main.c). A fragment carries rules of its own,
# so `all` is a goal and the run builds where refusing would have built nothing.
#
# Having found none of its default names on disk, GNU Make also puts all three
# on the chain to be remade if any rule can: RM_DONTCARE, "bail out as soon as
# this file is made, and main not to die if we can't make this file" (read.c).
# The `%:` fragment is such a rule, which is why GNUmakefile, makefile and
# Makefile are each reached here — and why an empty directory with no such rule
# stays silent rather than complaining about three files nobody asked for.
#
# The recipes are written without a space in them because a case's `args` are
# whitespace-separated words with no quoting; `:>FILE` is the shortest recipe
# that leaves evidence.
this-file-is-not-read := true
