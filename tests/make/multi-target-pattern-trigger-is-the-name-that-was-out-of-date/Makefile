# `$@' is the name whose own state reached the rule, which need not be the
# first one written: `a.c' is current, so it is `a.h' that runs the recipe.
all: a.c a.h
%.c %.h: %.in ; echo 'at=$@' >> log ; cat $< > $*.c ; cat $< > $*.h
