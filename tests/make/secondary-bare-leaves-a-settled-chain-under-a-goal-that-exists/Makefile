# The control for the case beside this one: with the goal on disk there is
# nothing that must be made, and the absent intermediates stay absent.
.SECONDARY:
all: hello.tsk
hello.x: ; echo body > $@
%.z: %.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
