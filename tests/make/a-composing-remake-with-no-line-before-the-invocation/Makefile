# The other side of the same shape, and the boundary of the defect: the
# `$(MAKE)` line is the recipe's first, so there is nothing to stage ahead of
# it and the composition finishes in one pass. The makefile update then has a
# wrapper edge that carries a rule rather than the freshness probe, which is
# what the shape above did not.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@$(MAKE) -C sub marker
	@printf 'GENERATED := from-child\n' > $@
