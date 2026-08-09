.SECONDEXPANSION:
P = dep
all: a.x

%.x: $$(P) ; @printf 'first\n' > log; touch $*.x
%.x:      $$(P) ; @printf 'second\n' > log; touch $*.x

dep:
