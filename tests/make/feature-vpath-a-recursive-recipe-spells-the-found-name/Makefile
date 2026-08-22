# A recipe composing a recursive `$(MAKE)` is one the compiler has to read
# before the build, so the names in it are references until the build settles
# which of a searched-for prerequisite's two names stands.
VPATH = build

all: out.o ; @printf 'whole [%s] first [%s]\n' '$^' '$<' > seen; $(MAKE) -f sub.mk

out.o: out.c ; @printf 'made\n' > $@
