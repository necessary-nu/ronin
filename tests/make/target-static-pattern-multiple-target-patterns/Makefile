all: a.o
a.c: ; echo body > $@
a.o: %.o %.q: %.c ; printf 'stem=[%s]\n' '$*' > $@
