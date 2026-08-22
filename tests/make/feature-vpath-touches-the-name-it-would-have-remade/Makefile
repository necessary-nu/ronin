# `-t` stands in for the recipes a build would have run, and it stands in for
# them where the build would have written: the target the search found is stale
# there, so the name that gets a date is the bare one and the found copy is
# left alone.
VPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
