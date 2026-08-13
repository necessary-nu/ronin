.INTERMEDIATE: hello.z
.PRECIOUS: %.z
all: hello.tsk
hello.x: ; echo body > $@
%.z: %.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
