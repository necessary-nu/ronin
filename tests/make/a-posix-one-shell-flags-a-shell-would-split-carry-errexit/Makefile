# The second of the three words is the flags GNU Make defaults to for a launch it
# was handed none: `posix_pedantic && NONE_SET (flags, COMMANDS_NOERROR) ? "-ec"
# : "-c"` (job.c). The recursion that parses `.SHELLFLAGS` passes none, so under
# `.POSIX:` the word is `-ec` even though this Makefile assigned `.SHELLFLAGS`
# and got no `-e` of its own from it.
#
# Only the line's own `-` clears COMMANDS_NOERROR. Measured on 4.4.1: `.IGNORE:`
# beside `.POSIX:` still gives `-ec`, and a `-` written on the recipe line gives
# `-c`.
.POSIX:
.ONESHELL:
SHELL = ./shell
.SHELLFLAGS = "-e" -c
all:
	echo hello
