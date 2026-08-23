# A pattern-specific `SHELL` reaches the target the pattern matched, which is
# the same lookup by another route: GNU Make reads `$(SHELL)` through the
# file's own variable set and a pattern-specific assignment is in it. A shell
# read once for the whole graph answers `/bin/sh` here.
%.o: SHELL := ./shell
all: x.o
x.o:
	@echo pattern > pattern.txt
