all: out
gate: ; echo gate > $@
src: ; echo src > $@
out: src | gate
	printf '[%s][%s][%s]\n' '$<' '$^' '$|' > $@
