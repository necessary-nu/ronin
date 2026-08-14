.SECONDEXPANSION:
all: mismatch
present.c: ; echo body > $@
mismatch: %.o: $$*.c present.c ; printf 'stem=[%s] all=[%s]\n' '$*' '$^' > $@
