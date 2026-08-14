# A target found by the search and already current there is not remade at all,
# and the name its dependents read is the one the search returned. The VPATH
# name is only thrown away by a target that has to be remade.
VPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf '%s from %s\n' '$@' '$<' > $@
