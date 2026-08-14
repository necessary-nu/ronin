# One pattern rule spelling two target patterns is one recipe that makes both,
# so asking for both runs it once, and `$@' is the target that reached it.
all: hello.tab.c hello.tab.h
hello.y: ; echo body > $@
%.tab.c %.tab.h: %.y ; echo 'at=$@ stem=$*' >> log ; cat $< > $*.tab.c ; cat $< > $*.tab.h
