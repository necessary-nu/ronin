.SECONDEXPANSION:
hello.src: ; echo body > $@
all: hello.out
hello.out: %.out: $$*.src
	cat $< > $@
