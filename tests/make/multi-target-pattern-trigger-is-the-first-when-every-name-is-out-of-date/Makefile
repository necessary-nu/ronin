# Nothing is there, so the first name the walk reaches is also the first
# that has to be made — which is why a build from scratch never shows the
# question this node is about.
all: a.c a.h
%.c %.h: %.in ; echo 'at=$@ stem=$*' >> log ; cat $< > $*.c ; cat $< > $*.h
