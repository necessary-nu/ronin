# `&:' on a pattern rule asks for what several target patterns already mean, so
# the group marker changes nothing about how the recipe runs or what `$@' is.
all: hello.tab.c hello.tab.h
hello.y: ; echo body > $@
%.tab.c %.tab.h &: %.y ; echo 'at=$@ stem=$*' >> log ; cat $< > $*.tab.c ; cat $< > $*.tab.h
