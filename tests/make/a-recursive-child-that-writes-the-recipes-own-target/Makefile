# The same question asked of the child rather than of the staged lines: the
# Make this recipe starts writes the target the recipe is for, so the file is
# on the ground before the line after the invocation is reached. The recipe
# still has a line left and GNU Make still runs it, and what the target holds
# is the child's answer with that line appended.
pre.out:
	@echo before > staged.txt
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
