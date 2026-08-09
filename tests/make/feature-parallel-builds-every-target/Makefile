# -j changes when recipes run, not which: every prerequisite is still made.
all: a.out b.out c.out

a.out b.out c.out: ; @printf '%s\n' '$@' > $@
