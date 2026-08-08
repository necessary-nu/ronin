all: main.o
main.c: ; echo source > $@
gate.h: ; echo gate > $@
%.o: %.c|gate.h
	printf 'made %s from %s order %s\n' '$@' '$<' '$|' > $@
