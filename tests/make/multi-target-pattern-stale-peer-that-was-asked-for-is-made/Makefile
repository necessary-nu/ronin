# Staleness is read off the name that was asked for, not off the group: `a.c'
# is newer than the prerequisite and `a.h' is older, and the recipe runs.
all: a.c a.h
%.c %.h: %.in ; echo ran >> log ; cat $< > $*.c ; cat $< > $*.h
