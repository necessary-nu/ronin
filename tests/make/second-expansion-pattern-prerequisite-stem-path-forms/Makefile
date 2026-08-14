# The `D` and `F` forms of the stem are readable from a deferred prerequisite.
# GNU Make defines them once as recursive variables over `$*`, so they answer
# for whatever the search bound the stem to.
.SECONDEXPANSION:
all: lib/bye.x

%.x: $$(*F).c $$(*D).c ; @printf '%s\n' '$^' > log; mkdir -p $(@D); touch $@

.PHONY: bye.c lib.c
