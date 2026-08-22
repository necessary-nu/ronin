# The boundary of what a reference can do. A word standing for a name the build
# has still to settle reaches the command whole, and a function handed one
# reads a word whose content is not the name: `$(filter %.o,$^)` would match
# nothing and `$(basename $^)` would cut nothing off. So a value a function is
# about to read carries the name as written instead.
VPATH = build

all: out.o plain.h ; @printf 'whole [%s] filtered [%s] based [%s] new [%s]\n' '$^' '$(filter %.o,$^)' '$(basename $^)' '$?' > seen

out.o: out.c ; @printf 'made\n' > $@
