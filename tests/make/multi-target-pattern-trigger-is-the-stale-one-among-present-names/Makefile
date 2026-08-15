# Both names are there and only a.h is behind a.in, so a.h is what sends
# the recipe to the shell and what $@ names.
all: a.c a.h
%.c %.h: %.in ; echo 'at=$@ stem=$*' >> log ; cat $< > $*.c ; cat $< > $*.h
