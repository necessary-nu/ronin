# A target may be named by several rules; the prerequisites of all of them are
# collected, and at most one rule carries the recipe.
all: foo.out bar.out

foo.out: one.h
bar.out: one.h two.h

foo.out bar.out: base.h ; @printf '%s from %s\n' '$@' '$^' >> log; printf 'built\n' > $@
