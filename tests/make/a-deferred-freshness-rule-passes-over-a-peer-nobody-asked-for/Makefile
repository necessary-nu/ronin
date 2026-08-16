# The other half of the same division, and the reason the fix is a filter
# rather than "every output". `a.h' is absent, but nothing asked for it: it is
# a peer, so no state of its can send the recipe to the shell. `b.c' and `b.h'
# were both asked for and are both current. Nothing runs.
all: a.c b.c b.h
%.c %.h: %.in ; echo "$* q=$?" >> log ; cat $< > $*.c ; cat $< > $*.h
