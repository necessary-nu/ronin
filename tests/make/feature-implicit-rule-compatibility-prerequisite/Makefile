# GNU Make's compatibility pass. When every rule has failed, the search runs
# again and takes a prerequisite the Makefile merely writes down somewhere on
# trust rather than testing it. `foo` is named on `all` and nowhere else, so
# `%.out: %` may use it even though the search cannot find a way to make it:
# `%: %.in` is a match-anything rule and may not make an intermediate.
all: foo foo.out

%: %.in ; @printf '%s\n' made-$@ > $@.made
%.out: % ; @printf '%s\n' made-$@ > $@.made
foo.in: ; @printf '%s\n' in > foo.in
