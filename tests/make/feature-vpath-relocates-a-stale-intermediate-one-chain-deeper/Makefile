# The same throwing away in the middle of a chain: the search finds inter.b in
# work/, it is out of date there, so it is made here and what reads it reads
# the bare name. The copy the search found is left where it was.
VPATH = work
.SECONDARY:

all: final.a ; @printf 'all sees %s\n' '$^' > seen

final.a: inter.b ; @printf 'final from %s\n' '$<' > $@
inter.b: final.src ; @printf 'inter from %s\n' '$<' > $@
