# When two patterns match the same target, the one that leaves the shorter stem
# is the one whose value is used.
%-mt.x: which := longer-pattern
%.x: which := shorter-pattern

all: foo.x foo-mt.x

foo.x foo-mt.x: ; @printf '%s\n' '$(which)' > $@
