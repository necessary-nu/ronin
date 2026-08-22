# `update_goal_chain` (remake.c) sets -q aside while the Makefiles are remade, so
# the pass that invents gen.z runs its recipe for real. `remove_intermediates`
# (file.c) reads the switch back at the end of the run, where it is set again,
# and gives up on the whole sweep.
include gen.mk

all: ; @printf 'all\n' > out

%.mk: %.z ; @cat $< > $@
%.z: %.x ; @cat $< > $@
