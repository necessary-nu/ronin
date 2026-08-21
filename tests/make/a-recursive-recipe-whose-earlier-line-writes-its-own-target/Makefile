# The lines of a recursive recipe written ahead of its `$(MAKE)` are run
# separately, because the child Makefile is read off the disk they write to —
# and one of them writes the target this very recipe makes. Reading the target
# afterwards says it exists with nothing newer behind it, which is the recipe's
# own work being mistaken for a reason not to run the recipe. GNU Make decides
# once, before the first line, and then runs all three.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
