# The same files as the case this node is named for, with the goals the
# other way round: a.h is reached first and is the one out of date, so both
# readings agree and $@ is a.h for two reasons rather than one.
all: a.h a.c
%.c %.h: %.in ; echo 'at=$@ stem=$*' >> log ; cat $< > $*.c ; cat $< > $*.h
