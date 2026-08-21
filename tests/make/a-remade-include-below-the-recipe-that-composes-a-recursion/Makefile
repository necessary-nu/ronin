# Two makefiles to remake, one of them by a recipe that composes and one by an
# ordinary recipe, and the includes written below both rules rather than above
# them. Where the include sits decides nothing: GNU Make reads the whole file
# before it updates any of them, so both are remade and the read starts over
# with both texts in it.
all: ; @printf '%s %s\n' '$(GENERATED)' '$(PLAIN)' > out

gen.mk:
	@printf 'GENERATED := from-generated\n' > $@
	@$(MAKE) -C sub marker

plain.mk:
	@printf 'PLAIN := from-plain\n' > $@

include gen.mk
include plain.mk
