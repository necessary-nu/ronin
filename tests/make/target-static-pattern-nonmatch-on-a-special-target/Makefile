all: mismatch
.PRECIOUS: %.o: %.c
mismatch: %.o: %.c ; printf 'stem=[%s] all=[%s]\n' '$*' '$^' > $@
