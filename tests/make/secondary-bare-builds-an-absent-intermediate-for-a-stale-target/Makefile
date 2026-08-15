# The same shape with hello.tsk behind hello.x: what reads the excused file
# has to run, so the file is asked for after all.
.SECONDARY:
all: hello.tsk
hello.x: ; echo body > $@
%.z: %.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
