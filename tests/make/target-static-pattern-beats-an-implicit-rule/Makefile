all: a.o
a.c: ; echo from-c > $@
a.q: ; echo from-q > $@
%.o: %.q ; cat $< > $@
a.o: %.o: %.c ; cat $< > $@
