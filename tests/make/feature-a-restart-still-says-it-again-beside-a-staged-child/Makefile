# The other half, and the one a fix could break by simply never speaking
# twice. A remade Makefile starts the read over on text that is new, so GNU
# Make says everything again and `MAKE_RESTARTS` is there so a Makefile can
# tell which time it is. Two recursive children stage passes around that
# restart and add nothing to what is said.
$(file >> seen,a read happened R=$(MAKE_RESTARTS))

include gen.mk

gen.mk: ; @printf 'V := built\n' > $@

all:
	@$(MAKE) --no-print-directory -f first.mk sub
	@$(MAKE) --no-print-directory -f second.mk sub
	@printf 'value=[%s]\n' '$(V)' > out
