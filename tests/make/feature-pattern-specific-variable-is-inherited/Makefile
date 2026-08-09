# A pattern-specific value reaches the prerequisites of the target it matched,
# the same way a target-specific one does.
all: four.x

%.x: PVAR = from-pattern

four.x: dep ; @printf '%s\n' '$(PVAR)' > four-out
dep: ; @printf '%s\n' '$(PVAR)' > dep-out
