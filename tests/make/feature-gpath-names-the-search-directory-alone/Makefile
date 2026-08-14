# The directory GPATH is asked about is the one the search joined to the name,
# not the whole prefix that join produced: a target carrying a directory of its
# own is still answered by the search directory.
VPATH = build
GPATH = build

all: sub/out.o ; @printf 'all sees %s\n' '$^' > seen

sub/out.o: sub/out.c ; @printf '%s from %s\n' '$@' '$<' > $@
