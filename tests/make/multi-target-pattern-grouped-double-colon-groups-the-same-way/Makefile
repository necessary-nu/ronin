# `&::' on a pattern rule reaches the same grouping, and one record of it runs
# once for both names.
all: hello.tab.c hello.tab.h
hello.y: ; echo body > $@
%.tab.c %.tab.h &:: %.y ; echo 'at=$@' >> log ; cat $< > $*.tab.c ; cat $< > $*.tab.h
