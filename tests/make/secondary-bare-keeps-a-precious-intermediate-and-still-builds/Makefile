# A bare .SECONDARY beside a .PRECIOUS pattern: the chain still runs, and
# neither reason to keep hello.z is a reason not to make it.
.SECONDARY:
.PRECIOUS: %.z
all: hello.tsk
hello.x: ; echo body > $@
%.z: %.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
