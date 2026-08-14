all: mismatch a.o
a.c a.dep: ; echo body > $@
a.o mismatch: %.o: %.c | %.dep ; printf 'name=[%s] all=[%s] order=[%s]\n' '$@' '$^' '$|' > $@
