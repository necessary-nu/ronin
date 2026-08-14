all: lib.a
prefix.c: ; echo body > $@
lib.a: lib%.a: prefix%.c ; printf 'stem=[%s] first=[%s]\n' '$*' '$<' > $@
