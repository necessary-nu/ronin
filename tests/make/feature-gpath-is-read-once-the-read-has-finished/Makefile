# GPATH is read after the whole Makefile has been, so it reaches a variable the
# lines below it define.
VPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@

GPATH = $(kept)
kept = build
