all: one.x two.y

%.x: $$x ; @printf 'one-immediate\n' >> log; touch $*.x
%.y: $$x tail ; @printf 'two-immediate\n' >> log; touch $*.y

.SECONDEXPANSION:
%.x: $$x ; @printf 'one-deferred\n' >> log; touch $*.x
%.y: $$x tail ; @printf 'two-deferred\n' >> log; touch $*.y
