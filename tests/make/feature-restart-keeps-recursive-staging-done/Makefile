# A restart is not a different build. The work staged so a recursive child can
# be evaluated at all — here the first sub-make of a pair, which has to have
# run before the second can be read — is still recognised as done after the
# read starts again for a remade Makefile.
all:
	@$(MAKE) -f mid.mk

-include gen.mk

gen.mk: ; @printf 'V := built\n' > $@
