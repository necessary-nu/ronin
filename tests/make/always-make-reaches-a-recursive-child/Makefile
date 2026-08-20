# `-B` travels in MAKEFLAGS the way every switch does, so a recursive child
# rebuilds everything too. Here the child's `out` is newer than its `src` and
# is rewritten all the same.
all: ; @printf 'top flags=[%s]\n' '$(MAKEFLAGS)' > log; $(MAKE) -C sub
