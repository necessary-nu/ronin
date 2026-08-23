# A `$` in `SHELL` is not expanded a second time — `construct_command_argv`
# expands the REFERENCE `$(SHELL)` and hands the result to the assembly as it
# stands — so the launch looks for a program whose NAME holds a dollar sign.
# GNU Make 4.4.1 does not find `./sh$X` and stops at 127.
#
# `./sh` is here and would write `pwned` if anything started it. Nothing does:
# the value names `./sh$X`, and a build that let the shell strip the reference
# would be running a program the makefile never named.
SHELL := ./sh$$X
all:
	@echo one > out
