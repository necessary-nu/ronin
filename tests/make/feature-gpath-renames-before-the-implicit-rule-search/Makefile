# The rename is the first thing that happens to the target, so the implicit
# rule search matches the path it was renamed to: the stem is build/out and the
# prerequisite it would need is build/out.c, which is not there.
VPATH = build
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

%.o: %.c ; @printf '%s from %s\n' '$@' '$<' > $@
