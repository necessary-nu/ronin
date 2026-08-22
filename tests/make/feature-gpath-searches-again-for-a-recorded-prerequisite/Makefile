# The prerequisite a static pattern rule recorded is a name like any other, so
# it is searched for in its own right: the target is made at the path GPATH
# kept, from the source the search found somewhere else again.
VPATH = build src
GPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: %.o: %.c ; @printf 'made [%s] from [%s] stem [%s]\n' '$@' '$<' '$*' > $@
