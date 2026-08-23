# `func_shell_base` reaches `construct_command_argv` like every other launch, so
# a `$(shell)` is assembled and escaped the same way. The `SHELL` goes back to
# the default afterwards, so the recipe below runs under a shell that exists and
# the case is about the expansion above it.
SHELL := ./sh$$(touch pwned)
V := $(shell echo hi)
SHELL := /bin/sh
all:
	@printf 'v=<%s>' '$(V)' > out
