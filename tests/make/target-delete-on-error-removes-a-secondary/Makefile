.DELETE_ON_ERROR:
.SECONDARY: hello.z
all: hello.tsk
hello.x: ; echo body > $@
%.z: %.x ; cat $< > $@ ; exit 1
%.tsk: %.z ; cat $< > $@
