# Every pattern that matches a target contributes, applied shortest pattern
# first — which is to say longest stem first — and the target's own setting
# lands last of all. `+=` appends to what the scopes before it left.
all: foo.x foo-mt.x sub/bar.x

%-mt.x: which += longer
%.x: which += shortpat
sub/%.x: which += subpat
foo.x: which += exact

foo.x foo-mt.x sub/bar.x: ; @printf '%s\n' '$(which)' > $@
