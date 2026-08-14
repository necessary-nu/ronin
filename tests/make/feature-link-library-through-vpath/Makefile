# The whole search, from GNU Make's own suite. Each reference is resolved
# through every element of `.LIBPATTERNS` — `lib%.so` then `lib%.a` — and the
# earliest answer wins across both. `-l2` is the cell that says what earliest
# means: `lib2.so` is in b2 and `lib2.a` is in a2, both reached by the same
# `vpath % a2 b2` entry, and a2 comes first within it.
vpath %.h b3
vpath %.a a1
vpath %.so b1
vpath % a2 b2
vpath % b3
all: -l1 -lc -l2 -l3; @printf '%s\n' '$^' > out
