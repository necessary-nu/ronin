# A recipe that writes one of the names it was given is not a failure: Make
# warns about the peer it did not update and the build succeeds.
all: a.c a.h
a.in: ; echo body > $@
%.c %.h: %.in ; cat $< > $*.c
