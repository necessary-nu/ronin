# GNU Make enters every environment variable as RECURSIVE — `define_variable_in_set
# (name, len, value, o_env, 1)` in main.c, where the trailing 1 is the recursive
# flag — so a `$` in an environment value is makefile syntax rather than bytes on
# their way to the shell. It is read at every reference, which is what makes the
# conditional below an answer: `3` is the expansion, and the text itself is what
# a reader that never expands hands back.
MADE_HERE = from-the-makefile

ifeq ($(COUNT),3)
all: the-value-was-read
else
all: the-value-was-bytes
endif

the-value-was-read:
	@printf 'count=[%s] value=[%s] flavor=[%s] origin=[%s]\n' '$(COUNT)' '$(value COUNT)' '$(flavor COUNT)' '$(origin COUNT)' > out
	@printf 'dollar=[%s] raw=[%s]\n' '$(DOLLAR)' '$(value DOLLAR)' >> out
	@printf 'env-to-env=[%s] env-to-makefile=[%s]\n' '$(INDIRECT)' '$(REACHES_MAKEFILE)' >> out
the-value-was-bytes:
	@printf 'branch=[the value was bytes]\n' > out
