# The command between two wildcards need not be on a line of its own: the
# second wildcard of a single expansion reads what the shell call beside it
# left behind.
seen := $(wildcard *.mid)$(shell printf 'x\n' > made.mid; printf '|')$(wildcard *.mid)

all: ; @printf '[%s]\n' '$(seen)' > out
