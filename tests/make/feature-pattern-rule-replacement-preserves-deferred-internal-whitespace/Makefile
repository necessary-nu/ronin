.SECONDEXPANSION:
P = dep
all: a.x

%.x: fixed  $$(P) ; @printf 'first\n' > log; touch $*.x
%.x: fixed $$(P) ; @printf 'second\n' > log; touch $*.x
