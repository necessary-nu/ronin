# The same Makefile with `foo` taken off `all`, which is the only place it was
# written down. The compatibility pass has nothing to take on trust, so the
# search runs out and the build is refused.
all: foo.out

%: %.in ; @printf '%s\n' made-$@ > $@.made
%.out: % ; @printf '%s\n' made-$@ > $@.made
foo.in: ; @printf '%s\n' in > foo.in
