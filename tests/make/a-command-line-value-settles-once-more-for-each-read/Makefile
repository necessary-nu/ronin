# The same value, read twice by the recipe: one settle for the environment and
# one per read, which is what makes the count a line rather than a constant. A
# tool that settled the definition twice would leave one line too many here and
# in the case beside this one alike.
GEN = $(shell ./gen)
all: ; @printf '%s %s\n' '$(V)' '$(V)' > out
