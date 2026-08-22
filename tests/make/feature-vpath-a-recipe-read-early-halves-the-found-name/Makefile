# The `D` and `F` forms are halves of the same names, so a recipe read early
# carries a reference per half rather than halving the reference.
VPATH = build

all: out.o ; @printf 'hatD [%s] hatF [%s] lessD [%s] lessF [%s] new [%s]\n' '$(^D)' '$(^F)' '$(<D)' '$(<F)' '$?' > seen

out.o: out.c ; @printf 'made\n' > $@
