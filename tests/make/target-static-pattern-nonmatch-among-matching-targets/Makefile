all: a.o mismatch b.o
a.c b.c: ; echo body > $@
a.o b.o mismatch: %.o: %.c ; printf 'name=[%s] stem=[%s] all=[%s]\n' '$@' '$*' '$^' > $@
