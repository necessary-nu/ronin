# A static pattern rule with an empty prerequisite pattern is legal, and the
# first of its targets is still the default goal.
foo.x bar.x: %.x: ; @printf '%s\n' '$*' > $@
