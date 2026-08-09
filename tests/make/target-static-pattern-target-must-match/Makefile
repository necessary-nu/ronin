all: mismatch
mismatch: %.o: %.c ; printf 'stem=[%s] first=[%s] all=[%s]\n' '$*' '$<' '$^' > $@
