# `$@' is the name whose own state reached the rule, and for a pattern group
# that is settled at launch, not while the graph is built: `a.c' is current, so
# `a.h' is the one out of date and `$@' is `a.h'. Naming `$?' in the same recipe
# must not change that -- a recipe naming the new inputs is deferred to launch
# like any other, so both `$@' and `$?' are read once the timestamps are known.
all: a.c a.h
%.c %.h: %.in ; @echo "at=$@ q=$?" >> log ; cat $< > $*.c ; cat $< > $*.h
