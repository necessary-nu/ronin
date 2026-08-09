all: a.o
a.c: ; echo body > $@
gate: ; echo gate > $@
a.o: %.o: %.c | gate ; cat $^ > $@
