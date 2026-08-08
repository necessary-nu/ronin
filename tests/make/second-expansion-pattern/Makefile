.SECONDEXPANSION:
all: hello.out
hello.src: ; echo body > $@
%.out: $$*.src
	cat $< > $@
