# The goals shuffle too, not only each target's prerequisites: GNU Make calls
# shuffle_goaldeps_recursive on the goal chain immediately before it updates it.
# The Makefiles the read had to remake are deliberately not in that shuffle,
# because they were brought up to date before it happened.
a: ; @printf '$@\n' >> order
b: ; @printf '$@\n' >> order
c: ; @printf '$@\n' >> order
