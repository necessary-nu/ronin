# GNU Make turns -n off across the makefile update and back on for the goals,
# because a Makefile it only pretended to remake is one whose contents it would
# then have to guess. The recipe here is cut into segments around its composed
# $(MAKE), and those segments are the Makefile's own recipe: gen.mk is written
# for real and the child runs for real, and only the goal is described.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@
	@$(MAKE) -C sub marker
