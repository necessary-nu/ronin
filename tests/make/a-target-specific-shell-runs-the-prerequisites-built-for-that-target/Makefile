# A target-specific variable descends to the prerequisites built for that
# target, and `SHELL` is no exception. `dep` is built for `all`, which set one,
# and runs under it; `aside` is built for `default`, which set none, and runs
# under the global one. Both shells write to the same file, so the recording
# holds which was asked for each recipe. A shell read once for the whole graph
# writes `./global` on every line.
SHELL := ./global
default: all aside
all: SHELL := ./own
all: dep
	@echo one > one.txt
dep:
	@echo two > two.txt
aside:
	@echo three > three.txt
