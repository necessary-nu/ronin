# A recipe's $(shell) is a value like any other: it reaches the functions
# around it, and a quoted one is substituted rather than left as shell syntax.
all: ; @printf '%s\n' '$(if $(filter yes,$(shell echo yes)),taken,missed)' '$(subst b,B,$(shell echo abc))' '$(intcmp 1,2,$(shell echo lt),eq,gt)' '[$(shell echo quoted)]' > out
