all: hello.tsk test.z
hello.x: ; echo hello > $@
test.x: ; echo test > $@
%.z: %.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
