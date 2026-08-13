.INTERMEDIATE: hello.z
.PRECIOUS: %.z
all: hello.tsk
hello.x: ; echo body > $@
hello.z: hello.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
