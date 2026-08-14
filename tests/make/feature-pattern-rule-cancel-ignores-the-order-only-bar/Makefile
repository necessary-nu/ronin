# The comparison reads prerequisite names as one chain, so which side of the
# bar a name was written on does not tell two rules apart.
%.gen: %.src | %.alt ; @printf '%s\n' built > $@
%.gen: %.src %.alt

all: foo.gen
