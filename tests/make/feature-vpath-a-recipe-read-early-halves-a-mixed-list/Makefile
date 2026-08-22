# A list holding both a name the build has still to settle and one it never had
# to: the reference is halved by choosing the half's own reference, and the name
# beside it is halved here and now.
VPATH = build

all: out.o sub/plain.h ; @printf 'hatD [%s] hatF [%s] new [%s]\n' '$(^D)' '$(^F)' '$?' > seen

out.o: out.c ; @printf 'made\n' > $@
