objects = a.o b.o
all: $(objects)
a.c: ; echo a > $@
b.c: ; echo b > $@
$(objects): %.o: %.c ; cat $< > $@
