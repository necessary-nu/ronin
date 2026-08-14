# The other half of the same rule: naming the directory the found path ends up
# in, rather than the one the search looked in, answers nothing.
VPATH = build
GPATH = build/sub

all: sub/out.o ; @printf 'all sees %s\n' '$^' > seen

sub/out.o: sub/out.c ; @printf '%s from %s\n' '$@' '$<' > $@
