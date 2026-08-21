# Two invocations in one recipe, so the segment between them is staged as well
# as the one ahead of the first: each child is read off what the one before it
# left. The first line writes the recipe's own target, which must not make the
# second invocation or the closing line look unnecessary, and the two children
# append in the order the recipe wrote them.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f a.mk a
	@$(MAKE) -f b.mk b
	@echo after >> pre.out
