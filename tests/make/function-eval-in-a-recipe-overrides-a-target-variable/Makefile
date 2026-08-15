# What a recipe assigns outranks what the makefile assigned to the same target.
V = outer
all: overridden
overridden: setter
overridden: V = from-makefile
setter:
	$(eval overridden: V = from-recipe)
	@printf 'set\n' > setter.out
overridden:
	@printf 'V=[%s]\n' '$(V)' > overridden.out
