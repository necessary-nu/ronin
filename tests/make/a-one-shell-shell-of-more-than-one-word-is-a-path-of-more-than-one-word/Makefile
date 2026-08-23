# `construct_command_argv_internal`'s slow path (job.c) says "SHELL may be a
# multi-word command" and builds `$(SHELL) $(.SHELLFLAGS) LINE` as text, then
# recurses to tokenise it — which is what makes `SHELL = ./shell -x` work for an
# ordinary recipe. The `if (one_shell)` branch above it does not: it writes
# `new_argv[0] = xstrdup (shell)` and execs that. So under `.ONESHELL` this is a
# path with a space in it, nothing starts, and nothing is made.
.ONESHELL:
SHELL = ./shell -x
all:
	echo one > one.txt
	echo two > two.txt
