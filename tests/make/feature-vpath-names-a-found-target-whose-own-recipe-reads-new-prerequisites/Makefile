# The found target's OWN recipe is the one naming `$?` here, which is what makes
# its edge answer for its freshness after its prerequisites settle rather than
# in the ordinary scan. It is still current where the search found it, so
# nothing runs and `all` reads the found path.
VPATH = build

all: out.o ; @printf 'all sees %s\n' '$^' > seen

out.o: out.c ; @printf 'made from [%s]\n' '$?' > $@
