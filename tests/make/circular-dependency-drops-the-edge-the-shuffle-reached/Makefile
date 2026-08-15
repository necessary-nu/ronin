# Which prerequisite closes the loop is a question about the order the walk
# reads the list in, and --shuffle is what changes that order. GNU Make takes
# `d = du->shuf ? du->shuf : du` before it asks whether the file is already
# being updated, so the shuffle chooses the edge: written order drops b_ <- a_
# and builds b_ first, while reverse drops a_ <- b_ and builds a_ first.
all: a_ b_ ; @printf '$@\n' >> order
%_: ; @printf '$@\n' >> order
a_: b_
b_: a_
