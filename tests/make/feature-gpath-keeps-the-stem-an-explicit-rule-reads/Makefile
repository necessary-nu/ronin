# An explicit rule records no target pattern, so there is no stem to carry and
# `$*` is read the way Unix Make reads one: the target's own name with a known
# suffix taken off. That name is the one the rename left, directory and all.
VPATH = build
GPATH = build
.SUFFIXES: .o .c

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf 'made [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > $@
