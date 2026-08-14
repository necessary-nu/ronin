all: mismatch
mismatch: extra
extra: ; echo body > $@
mismatch: %.o: %.c ; printf 'stem=[%s] all=[%s]\n' '$*' '$^' > $@
