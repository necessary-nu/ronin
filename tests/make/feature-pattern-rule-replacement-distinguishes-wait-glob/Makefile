all: a.x
%.x: .W* ; @printf 'first\n' > log; touch $*.x
%.x: .WAIT ; @printf 'second\n' > log; touch $*.x
