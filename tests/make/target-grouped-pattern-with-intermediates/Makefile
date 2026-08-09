all: hello.tsk
hello.x: ; echo body > $@
%.z %.w &: %.x ; cat $< > $*.z ; cat $< > $*.w
%.tsk: %.z ; cat $< > $@
