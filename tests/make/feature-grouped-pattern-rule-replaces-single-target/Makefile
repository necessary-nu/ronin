# With equal prerequisites, a later grouped pattern rule containing the target
# of an earlier single-target pattern rule replaces it in GNU Make's rule list.
all: a.x

%.x: ; @printf 'first\n' > log; touch $*.x

%.x %.y: ; @printf 'second\n' > log; touch $*.x $*.y
