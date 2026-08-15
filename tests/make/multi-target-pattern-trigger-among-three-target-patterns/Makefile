# Three target patterns with the middle name the only one missing.
all: a.c a.h a.k
%.c %.h %.k: %.in ; echo 'at=$@ stem=$*' >> log ; cat $< > $*.c ; cat $< > $*.h ; cat $< > $*.k
