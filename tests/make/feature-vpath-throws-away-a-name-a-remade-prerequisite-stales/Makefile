# The cell that says the choice cannot be made when the Makefile is read. When
# the read ends, build/out.o is newer than out.c and a compile-time fold would
# keep the found name; out.c's own rule then runs and the answer flips, so the
# target is remade here and its dependents read the bare name.
VPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
out.c: gen.in ; @printf 'regenerated\n' > $@
