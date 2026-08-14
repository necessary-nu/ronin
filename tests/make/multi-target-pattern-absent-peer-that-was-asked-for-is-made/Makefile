# The same absence, asked about: `a.h' is a goal of its own, so it is remade
# however current the target beside it is.
all: a.c a.h
%.c %.h: %.in ; echo ran >> log ; cat $< > $*.c ; cat $< > $*.h
