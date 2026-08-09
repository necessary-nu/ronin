all: hello.o
hello.c: ; echo body > $@
hello.o: %.o: %.c ; printf '%s\n' 'stem=$*' 'first=$<' 'name=$@' > $@
