# `$(MAKE)` is one word and carries nothing, so the only way a `--eval`
# fragment reaches a child is MAKEFLAGS. GNU Make writes it there quoted — a
# `$` doubled so reading the variable does not expand it — and every Make below
# reads it back before its own makefile. That is why the child's recipe can use
# a variable no makefile in this tree defines, and why the `$` in it survives
# the trip instead of arriving as an expansion.
all:
	@$(MAKE) --no-print-directory -C sub child
	@printf 'parent=[%s]\n' '$(FROM_EVAL)' > out
