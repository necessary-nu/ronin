# The same escaping, through the other substitution syntax and with a
# redirection for its payload — a backquote and a `>` are both characters
# special to the shell, so both get a backslash and neither is the shell's to
# read.
SHELL := ./sh`>pwned`
all:
	@echo one > out
