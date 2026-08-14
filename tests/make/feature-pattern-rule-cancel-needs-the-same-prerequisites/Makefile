# A recipe-less pattern rule withdraws only a rule written the same way it is.
# The prerequisite patterns are part of that, so this one withdraws nothing and
# the rule above it still makes the target.
%.gen: %.src ; @printf '%s\n' built > $@
%.gen: %.alt

all: foo.gen
