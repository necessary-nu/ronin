# A rule with several targets is the same as one rule per target, so the recipe
# runs once for each with its own $@.
all: big.out little.out

big.out little.out: shared.h ; @printf '%s\n' '$(subst .out,,$@)' > $@
