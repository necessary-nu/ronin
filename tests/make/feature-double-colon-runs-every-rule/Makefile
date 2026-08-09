# A double-colon target may carry several recipes, and each of them is a rule
# in its own right: all of them run, in the order they were read.
all: both

both:: ; @printf '%s\n' first >> log
both:: ; @printf '%s\n' second >> log
