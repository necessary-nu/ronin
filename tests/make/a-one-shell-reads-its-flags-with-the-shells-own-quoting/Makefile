# GNU Make's one-shell branch parses `.SHELLFLAGS` with
# `construct_command_argv_internal` itself — "Parse shellflags using
# construct_command_argv_internal to handle quotes" (job.c) — so a quoted flag
# with blanks in it is ONE argument and the quotes come off. `./shell` writes
# each argument on a line of its own, so the recording holds the split.
.ONESHELL:
SHELL = ./shell
.SHELLFLAGS = -w -E 'one two' -E
all:
	echo hello
