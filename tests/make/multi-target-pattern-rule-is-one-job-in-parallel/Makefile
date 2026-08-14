# Two outputs from one recipe is one job however many of them were asked for.
all: hello.tab.c hello.tab.h
hello.y: ; echo body > $@
%.tab.c %.tab.h: %.y ; echo ran >> log ; cat $< > $*.tab.c ; cat $< > $*.tab.h
