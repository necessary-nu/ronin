marker: ; @printf '%s\n' '$(CHILDGEN)' > marker.out

include child.mk

child.mk:
	@printf 'CHILDGEN := from-child\n' > $@
