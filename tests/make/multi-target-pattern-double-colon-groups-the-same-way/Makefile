# A `::' pattern rule is the terminal kind rather than a second record, so
# several target patterns on one still mean one recipe for all of them.
all: hello.tab.c hello.tab.h
hello.y: ; echo body > $@
%.tab.c %.tab.h:: %.y ; echo 'at=$@' >> log ; cat $< > $*.tab.c ; cat $< > $*.tab.h
