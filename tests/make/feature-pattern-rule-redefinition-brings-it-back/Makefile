# Cancelling is a replacement and not a memory of one: the recipe-less rule
# takes the name, and a third rule of the same name takes it from there.
%.gen: %.src ; @printf '%s\n' first > $@
%.gen: %.src
%.gen: %.src ; @printf '%s\n' second > $@

all: foo.gen
