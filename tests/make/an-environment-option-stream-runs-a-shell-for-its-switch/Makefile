# A `$(shell)` in an environment stream runs a command before any makefile is
# read, against the environment's own SHELL — GNU Make runs it here too, in
# `decode_env_switches`, before the switches that would construct the session.
# Its output `-k` is then split into switches like any other expanded text, so
# `good` runs after `bad` fails.
all: bad good

bad:
	@exit 1

good:
	@printf 'the shell produced -k\n' > kept-going
