# GNU Make does not fold a recipe. `construct_command_argv_internal` (job.c)
# tests for a backslash before the newline above its end-of-line case and writes
# the pair out, so the shell is the one that decides what they mean -- and
# inside single quotes it decides they are two characters of text.
all:
	@printf '%s' 'a\
	b' | od -An -c > out
