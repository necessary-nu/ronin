# One rule, two targets, and only one of them found elsewhere. Each keeps the
# prerequisite its own name derived, so the renamed target reads out.c and the
# one that stayed here reads two.c.
VPATH = build
GPATH = build

all: out.o two.o ; @printf 'all sees %s\n' '$^' > seen

out.o two.o: %.o: %.c ; @printf 'made [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > $@
