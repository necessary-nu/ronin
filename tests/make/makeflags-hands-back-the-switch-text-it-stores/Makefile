# What `$(MAKEFLAGS)` reads back, where a switch value carries a literal `$`.
#
# GNU Make's binding is recursive over the quoted text, so reading it halves
# every doubled `$` — `$(MAKEFLAGS)` and `$(value MAKEFLAGS)` are two different
# strings. Ronin's binding holds the stored text and reads back exactly that,
# so the two agree with each other, and with GNU Make's `$(value MAKEFLAGS)`,
# and not with GNU Make's `$(MAKEFLAGS)`.
#
# The difference is recorded rather than removed, for the reason the sibling
# case `a-child-keeps-a-switch-value-a-dollar-was-written-in` measures: GNU
# Make's halving is what loses the switch on the way to a child.
$(file > out,mf=[$(MAKEFLAGS)])

all:
	@printf 'ran\n' > ran
