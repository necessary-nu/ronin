# `include` says the file has to be there, and that carries into the rule that
# would have made it: the recipe runs, it fails, and the build stops without
# ever reaching the goal.
all: ; @printf 'all ran\n' > out

include gen.mk

gen.mk: ; @printf 'gen ran\n' > log; exit 1
