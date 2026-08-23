# The control on the escaping: a blank is NOT one of the characters GNU Make
# escapes, so it still ends a word and "SHELL may be a multi-word command" keeps
# working. A build that escaped everything the shell reads would be looking for
# one program named `/usr/bin/env sh`.
SHELL := /usr/bin/env sh
all:
	@echo one > out
