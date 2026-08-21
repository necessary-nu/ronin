# The nested remaking recipe's child has a makefile of its own to remake, so it
# runs the same two phases the parent does. Both are reached with the pretending
# switches off, and what the child's own goal reads out of its remade makefile
# is what says the whole chain ran for real.
all: ; @printf '%s\n' '$(GENERATED)' > out

include gen.mk

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@; $(MAKE) --no-print-directory -C sub marker
