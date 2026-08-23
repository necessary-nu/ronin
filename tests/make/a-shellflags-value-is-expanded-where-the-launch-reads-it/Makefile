# GNU Make expands the `.SHELLFLAGS` VALUE a second time, where the launch reads
# it: `construct_command_argv` (job.c) ends the lookup with
# `allocated_variable_expand_for_file (var->value, file)`. A simply-expanded
# value was already expanded once when it was assigned, so the `$` its
# assignment left behind is expanded AGAIN here — and `X` therefore resolves to
# what it holds when the recipe runs, not to what it held on the line above.
#
# `./shell` writes each argument on a line of its own, so the recording holds
# the word a build that read the value where it was written would not have.
X := one
SHELL = ./shell
.SHELLFLAGS := -e $$X -c
X := two
all:
	@echo hello
