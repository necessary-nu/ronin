# Recursive means lazy, and an environment value is no exception: GNU Make
# expands it where the reference is written, so a name nothing refers to is
# never read — not its `$(error)`, not its `$(shell)`, not its unterminated
# call — and a name referred to twice runs its `$(shell)` twice. Expanding the
# environment once at startup would answer every one of those differently, and
# would refuse this makefile outright.
COUNTED := $(TICK)$(TICK)

all:
	@printf 'counted=[%s]\n' '$(COUNTED)' > out
	@if [ -f eagerly-read ]; then printf 'eager\n' >> out; else printf 'lazy\n' >> out; fi
	@cat ticks >> out
