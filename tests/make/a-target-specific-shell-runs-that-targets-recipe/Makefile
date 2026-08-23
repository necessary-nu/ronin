# GNU Make expands `$(SHELL)` once per recipe with the target's own scope, so a
# `SHELL` set for one target is the shell that target's recipe runs under.
# `./shell` writes its own name and arguments to a file and then runs the
# script, so the recording holds who was asked as well as what happened.
all: SHELL := ./shell
all:
	@echo one > one.txt
