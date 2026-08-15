# hello.z is gone and hello.tsk is current, so its absence is excused and
# nothing runs — the half of the intermediate machinery a bare .SECONDARY
# does not change.
.SECONDARY:
all: hello.tsk
hello.x: ; echo body > $@
%.z: %.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
