# GPATH answers a question the directory search asked. Nothing found means
# nothing to answer, so the target is made here as it always was.
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
