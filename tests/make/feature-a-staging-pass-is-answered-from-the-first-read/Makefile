# What a read is TOLD cannot be held back the way what it DOES can: the
# expansion that asked has to be handed something. GNU Make asks once, before
# any recipe has run, so its answers describe the ground the build started
# with. Two recursive children make the compiler read this text again once the
# first child's inputs are staged, and by then the first child has run and the
# ground has moved. Every answer below must still be the first read's.
#
# The `$(shell)` is the half with an effect as well as an answer: running it
# again would append a second line to shelllog, which GNU Make never does.
X := $(shell printf 'asked\n' >> shelllog; echo told)
W := $(wildcard *.out)
R := $(realpath first.out)
F := $(file < first.out)
P := $(patsubst %,seen-%,$(wildcard *.out))
all:
	@$(MAKE) --no-print-directory -f first.mk sub
	@$(MAKE) --no-print-directory -f second.mk sub
	@printf 'X=[$(X)] W=[$(W)] F=[$(F)] P=[$(P)]\n' > answers
	@printf 'R-is-empty=[$(if $(R),no,yes)]\n' >> answers
