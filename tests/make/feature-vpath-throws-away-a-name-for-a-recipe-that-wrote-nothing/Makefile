# Whether the found name survives is decided by whether the target had to be
# remade, not by what the remaking left behind. This recipe runs and writes no
# file at all, and the dependents still read the bare name.
VPATH = build

all: out.o ; @printf 'all sees [%s] first [%s]\n' '$^' '$<' > seen

out.o: out.c ; @printf 'ran\n' > ran
