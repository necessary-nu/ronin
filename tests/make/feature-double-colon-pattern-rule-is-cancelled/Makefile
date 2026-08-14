# Which colon a pattern rule was written with is no part of the name the
# comparison reads, so a single-colon recipe-less rule withdraws a double-colon
# one.
%.gen:: %.src ; @printf '%s\n' built > $@
%.gen: %.src

all: foo.gen
