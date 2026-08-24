# What is in scope while an environment stream is expanded: the environment,
# and — this early, before `define_default_variables` and
# `define_automatic_variables` — nothing else. So `MAKEFLAGS=$(CARRIED)` with
# `CARRIED=-k` in the environment resolves to `-k`, and `$(CC)` or
# `$(MAKELEVEL)` would resolve to nothing.
#
# The `-k` keeps `good` running after `bad` fails, which is what `kept-going`
# reports.
all: bad good

bad:
	@exit 1

good:
	@printf 'the reference resolved to -k\n' > kept-going
