# The other direction of that comparison: a one-target recipe-less rule does
# not reach a rule that makes two names, so it still makes this one.
%.gen %.log: %.src ; @printf '%s\n' built > $@
%.gen: %.src

all: foo.gen
