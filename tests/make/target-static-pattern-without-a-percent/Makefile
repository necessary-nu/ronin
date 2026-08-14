all: a.o
a.c: ; echo body > $@
a.o: literal: %.c ; printf 'stem=[%s]\n' '$*' > $@
