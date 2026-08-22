# The other side of the same fork: a prerequisite the build had to remake is
# made where it is named, so the reference resolves to the name as written.
VPATH = build

all: out.o ; @printf 'whole [%s] first [%s] new [%s]\n' '$^' '$<' '$?' > seen

out.o: out.c ; @printf 'made\n' > $@
