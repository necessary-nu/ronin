# The peer nothing asked for is not part of deciding whether the recipe runs:
# `a.c' is current, so its absence leaves the build with nothing to do.
all: a.c
%.c %.h: %.in ; echo ran >> log ; cat $< > $*.c ; cat $< > $*.h
