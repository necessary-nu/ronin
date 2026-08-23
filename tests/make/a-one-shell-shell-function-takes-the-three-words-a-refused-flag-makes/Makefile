# The third of the one-shell branch's behaviours reaches a `$(shell)` as well:
# where the tokenizer refuses `.SHELLFLAGS`, the flags come back as the three
# words `[/bin/sh, <the flags GNU defaults to>, <that text>]` and the branch
# copies all three in ahead of the command. A double quote is a shell character,
# which is what sends this value that way.
#
# `SHELL` goes back to the default before any recipe runs, because 4.4.1 reaches
# the three-word answer through a heap overflow and dies of it on a second
# launch through the same path. The `$(shell)` above is the measurement, and
# `./shell` writes each argument on a line of its own.
.ONESHELL:
SHELL = ./shell
.SHELLFLAGS = "-e" -c
V := $(shell echo hello)
SHELL = /bin/sh
.SHELLFLAGS = -c
all:
	@printf '%s' 'v=<$(V)>' > out
