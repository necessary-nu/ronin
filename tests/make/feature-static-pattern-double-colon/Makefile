# The static pattern form works with a double colon too, and the stem is
# substituted into the prerequisite pattern just as for a single colon.
all: a.tgt b.tgt

a.tgt b.tgt:: %.tgt : %.src ; @cat $< > $@
