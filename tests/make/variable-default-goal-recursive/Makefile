# An assignment that defers keeps deferring: the goal is whatever the
# expression says once the whole makefile has been read.
CHOSEN := second
.DEFAULT_GOAL = $(CHOSEN)

first: ; @printf '%s\n' 'first ran' > first.out
second: ; @printf '%s\n' 'second ran' > out
