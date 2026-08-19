# The two kinds of pass interleave, and the count has to tell them apart in
# both directions. One generated Makefile is remade, which is one restart and
# stays one; the pair of recursive children in the recipe each need the read to
# end and begin again, and neither adds to the count nor puts it back to where
# the first read found it.
all:
	@$(MAKE) --no-print-directory -f first.mk sub
	@$(MAKE) --no-print-directory -f second.mk sub
	@printf 'restarts=[%s] value=[%s]\n' '$(MAKE_RESTARTS)' '$(V)' > out

include gen.mk

gen.mk: ; @printf 'V := built\n' > $@
