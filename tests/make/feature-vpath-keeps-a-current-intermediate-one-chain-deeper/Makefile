# And the other way in the same chain: the copy the search found is current, so
# nothing is made here at all and final.a is built from the found path.
VPATH = work
.SECONDARY:

all: final.a ; @printf 'all sees %s\n' '$^' > seen

final.a: inter.b ; @printf 'final from %s\n' '$<' > $@
inter.b: final.src ; @printf 'inter from %s\n' '$<' > $@
