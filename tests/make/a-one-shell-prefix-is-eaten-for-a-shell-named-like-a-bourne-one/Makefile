# The control for the case beside this one: the same shell, the same recipe,
# and a name that is on GNU Make's list. `is_bourne_compatible_shell` (job.c)
# compares the basename and looks at nothing else, so `./dash` is a Bourne
# shell as far as the strip is concerned and the `@` below the first line never
# reaches it.
SHELL = ./dash
.ONESHELL:
all:
	touch m1
	@touch m2
	touch m3
