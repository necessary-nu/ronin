.PRECIOUS: hello.z
all: hello.tsk
hello.x: ; echo body > $@
%.z: %.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
