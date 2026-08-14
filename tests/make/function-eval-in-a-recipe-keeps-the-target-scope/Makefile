# The rule-specific assignment interrupts the recipe's own target scope, and has
# to put it back: the line after it still reads the target's variables.
all: TSV = target-value
all:
	@printf '%s\n' 'before [$(TSV)]' > before-out
	$(eval other: LOCAL = x)
	@printf '%s\n' 'after [$(TSV)]' > after-out
