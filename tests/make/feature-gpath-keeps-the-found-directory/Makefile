# Without GPATH a target found along VPATH is rebuilt in the current directory;
# GPATH says to rebuild it where it was found.
VPATH = build
GPATH = build

all: out.o

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
