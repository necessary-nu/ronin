all: mismatch
mismatch:: %.o: %.c ; printf 'stem=[%s] all=[%s]\n' '$*' '$^' > $@
