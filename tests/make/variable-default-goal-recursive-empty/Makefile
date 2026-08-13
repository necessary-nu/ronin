# Selection is armed by the text of .DEFAULT_GOAL rather than by what that
# text expands to, so an expression that produces nothing still disarms it and
# leaves the build with nothing to aim at.
EMPTY :=
.DEFAULT_GOAL = $(EMPTY)

first: ; @printf '%s\n' 'first ran' > out
