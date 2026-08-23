# Expanding SHELL reaches `$(shell)`, `$(shell)` has to know what shell to
# start, and asking that expands SHELL. GNU Make's guard is `v->expanding`
# (expand.c), set around the whole expansion wherever it goes, so the second
# entry is caught however far around the houses it came. Nothing is built.
SHELL = $(shell echo /bin/sh)
all:
	echo one > one.txt
