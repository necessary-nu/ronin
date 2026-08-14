# Prerequisites declared across the files accumulate on the one target, exactly
# as they would if the two files had been concatenated.
all: early
early: ; @printf '%s\n' early >> out
