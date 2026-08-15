# a.c is not asked for, so no state of its own can send the recipe to the
# shell — it is written on the way and never triggers. $@ is a.h even though
# a.c is the name the rule writes first.
all: a.h
%.c %.h: %.in ; echo 'at=$@ stem=$*' >> log ; cat $< > $*.c ; cat $< > $*.h
