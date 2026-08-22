# The same for -t, which `update_goal_chain` also sets aside for the Makefiles:
# gen.z is made rather than touched and is still there when the run ends.
include gen.mk

all: ; @printf 'all\n' > out

%.mk: %.z ; @cat $< > $@
%.z: %.x ; @cat $< > $@
