# A bare `export` names nothing, so it speaks for every variable the makefile
# defined from then on — and for the ones it defined before, because what the
# directive sets is consulted when a child is started rather than where it is
# written.
one = 1
export
two = 2
all: ; @printf 'one=%s two=%s\n' "$$one" "$$two" > out
