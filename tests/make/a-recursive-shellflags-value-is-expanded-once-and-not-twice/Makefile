# The second expansion is the value's FIRST for a recursive `.SHELLFLAGS`, which
# is why GNU Make can expand the value rather than the reference without
# expanding a recursive one twice. Here the assignment stores `-e $$X -c`
# unexpanded, the launch expands it once to `-e $X -c`, and the `$X` that
# survives is the shell's to read — so the argument is gone by the time
# `./shell` counts, and `ZZZ` never appears.
X := ZZZ
SHELL = ./shell
.SHELLFLAGS = -e $$X -c
all:
	@echo hello
