# -n asks about the goals, and a Makefile the command line named is one of them.
# GNU Make hands that file the switches the invocation gave rather than the ones
# it remakes Makefiles under, so the recipe is described and never runs.
all: ; @printf 'all\n' > out

-include gen.mk

gen.mk: ; @printf 'V := built\n' > $@
