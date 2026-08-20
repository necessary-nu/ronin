# Where the count is judged decides what has already happened when it fails.
# `handle_function' (function.c) expands every argument of an argument-expanding
# function and only then hands the count to `expand_builtin_function', which is
# the first thing that looks at how many there are. So the arguments of a call
# about to be refused for having too few of them have all already run -- every
# one of them, not just the first.
FEW := $(subst $(shell touch the-first-argument-ran),$(shell touch the-second-argument-ran))

all: ; @echo built > out
