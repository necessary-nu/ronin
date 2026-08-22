# An order-only prerequisite is searched for like any other, and `$|` reads it
# off the same file object the rest do.
VPATH = build

all: plain.h | out.o ; @printf 'bar [%s] whole [%s] new [%s]\n' '$|' '$^' '$?' > seen

out.o: out.c ; @printf 'made\n' > $@
