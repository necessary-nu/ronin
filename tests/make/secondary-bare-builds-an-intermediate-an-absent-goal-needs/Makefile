# Every file here is intermediate, so nothing is out of date. The goal is
# not there, which is reason enough on its own to make what it reads.
.SECONDARY:
all: hello.tsk
hello.x: ; echo body > $@
%.z: %.x ; cat $< > $@
%.tsk: %.z ; cat $< > $@
